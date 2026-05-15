//! Quick Settings popup panel — opened by clicking the bar cluster.
//!
//! Renders three views (Main / Wi-Fi / Bluetooth), drives the brightness
//! and volume sliders, and owns the per-display brightness OSDs.

use super::bt::{bt_icon, bt_row_view, BtDevice, BtRowClick};
use super::osd::BrightnessOsd;
use super::system::{nmcli_set_wifi, rfkill_set, rfkill_set_all, spawn_lock_screen, spawn_power_menu, spawn_settings_app, toggle_night_light};
use super::widgets::{
    nav_row, pill, placeholder_row, render_battery_strip, secondary_header, section_label,
    slider_row,
};
use super::wifi::{scan_wifi_networks, signal_bars, wifi_row_view, WifiEntry};
use super::{Modules, PillsState, Back, Dismiss, PANEL_W, SLIDER_TRACK_LEFT_PX, SLIDER_TRACK_W_PX};
use crate::modules::volume::VolumeModule;
use crate::theme;
use gpui::{
    div, layer_shell::*, prelude::*, px, App, AppContext, Bounds, Context, DisplayId, FocusHandle,
    Focusable, MouseButton, Size, Window, WindowBackgroundAppearance, WindowBounds, WindowKind,
    WindowOptions,
};
use std::time::Duration;
use ztheme::Theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum View {
    Main,
    Wifi,
    Bluetooth,
}

#[derive(Default)]
pub(super) struct ScanState {
    pub(super) bt_devices: Vec<BtDevice>,
    pub(super) bt_scanned: bool,
    pub(super) wifi_networks: Vec<WifiEntry>,
    pub(super) wifi_scanned: bool,
    pub(super) wifi_available: Option<bool>,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub(super) enum SliderKind {
    /// `None` = laptop backlight / single-display fallback.
    /// `Some(id)` = one of N DDC/CI monitors (one slider per display).
    Brightness(Option<DisplayId>),
    Volume,
}

pub struct QuickSettingsPanel {
    pub(super) modules: Modules,
    pub(super) view: View,
    pub(super) pills: PillsState,
    focus_handle: FocusHandle,
    pub(super) scans: ScanState,
    /// Which slider (if any) is currently being dragged. Set by mouse_down
    /// on the track, used by mouse_move on the panel root, cleared by
    /// mouse_up on the panel root.
    pub(super) drag: Option<SliderKind>,
    /// Optimistic slider values shown while the user is dragging. Backing
    /// modules only re-read every few seconds, so without this the thumb
    /// would appear frozen during a drag. Keyed by display so each
    /// per-monitor brightness slider has its own latch.
    pub(super) drag_brightness: std::collections::HashMap<Option<DisplayId>, u32>,
    pub(super) drag_volume: Option<u32>,
    pub(super) bt_client: crate::bluetooth::BluetoothClient,
    /// Per-display brightness OSD overlays. Keyed by `DisplayId`; each
    /// entry is the live window plus its current state.
    pub(super) osd_windows: std::collections::HashMap<DisplayId, OsdHandle>,
}

pub(super) struct OsdHandle {
    pub(super) window: gpui::WindowHandle<BrightnessOsd>,
}

impl QuickSettingsPanel {
    pub(super) fn new(modules: Modules, cx: &mut Context<Self>) -> Self {
        cx.observe_global::<Theme>(|_, cx| cx.notify()).detach();
        cx.observe(&modules.volume, |_, _, cx| cx.notify()).detach();
        cx.observe(&modules.brightness, |_, _, cx| cx.notify()).detach();
        cx.observe(&modules.battery, |_, _, cx| cx.notify()).detach();

        // Spawn a BlueZ DBus client; drive its snapshot channel into our
        // device list. The client also subscribes to InterfacesAdded/Removed
        // so we update on external pair/unpair without polling.
        let bt_client = crate::bluetooth::BluetoothClient::shared();
        let snap_rx = bt_client.snapshots();
        cx.spawn(async move |this, cx| {
            while let Ok(devices) = snap_rx.recv().await {
                let mapped: Vec<BtDevice> = devices.into_iter().map(Into::into).collect();
                if this
                    .update(cx, |this: &mut Self, cx| {
                        this.scans.bt_devices = mapped;
                        this.scans.bt_scanned = true;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        // Poll system state every 2s while the panel is open so the pills
        // reflect external changes (e.g. user toggled wifi from the CLI).
        // The task ends naturally when the entity is dropped on close.
        cx.spawn(async move |entity, cx| loop {
            cx.background_executor()
                .timer(Duration::from_secs(2))
                .await;
            let probed = cx
                .background_executor()
                .spawn(async { PillsState::read_from_system() })
                .await;
            if entity
                .update(cx, |this: &mut Self, cx| {
                    if !this.pills.matches(&probed) {
                        this.pills = probed;
                        cx.notify();
                    }
                })
                .is_err()
            {
                break;
            }
        })
        .detach();

        Self {
            modules,
            view: View::Main,
            pills: PillsState::read_from_system(),
            focus_handle: cx.focus_handle(),
            scans: ScanState::default(),
            drag: None,
            drag_brightness: std::collections::HashMap::new(),
            drag_volume: None,
            osd_windows: std::collections::HashMap::new(),
            bt_client,
        }
    }

    /// Spawn a background scan for the requested view if we haven't already.
    /// Results land back on the GPUI thread via `cx.spawn`.
    fn ensure_scan_for_view(&mut self, cx: &mut Context<Self>) {
        match self.view {
            View::Bluetooth => {
                // Just ask the BlueZ client to push a fresh snapshot;
                // results arrive via the snapshots channel.
                self.bt_client.refresh();
            }
            View::Wifi if !self.scans.wifi_scanned => {
                self.scans.wifi_scanned = true;
                let entity = cx.entity().downgrade();
                cx.spawn(async move |_, cx| {
                    let (avail, entries) = cx
                        .background_executor()
                        .spawn(async { scan_wifi_networks() })
                        .await;
                    let _ = entity.update(cx, |this, cx| {
                        this.scans.wifi_available = Some(avail);
                        this.scans.wifi_networks = entries;
                        cx.notify();
                    });
                })
                .detach();
            }
            _ => {}
        }
    }

    fn dismiss(&mut self, _: &Dismiss, window: &mut Window, _cx: &mut Context<Self>) {
        window.remove_window();
    }

    fn back(&mut self, _: &Back, _window: &mut Window, cx: &mut Context<Self>) {
        if self.view != View::Main {
            self.view = View::Main;
            cx.notify();
        }
    }

    /// Estimated pixel-perfect layer-surface height for the current view.
    /// GPUI renders text with ~1.5 line-height so multi-line bodies are
    /// taller than naive `font_size * lines` math suggests. Numbers below
    /// use 1.5 as the line-height factor; a small fudge constant covers
    /// any remaining sub-pixel drift so the footer never clips.
    fn desired_height(&self, has_battery: bool, n_brightness: usize) -> f32 {
        let padding = 14.0 * 2.0;
        // Bottom-clip insurance — the layer surface must be at least a
        // few pixels taller than the measured content so the bottom
        // panel-padding never collapses to zero. Tuned empirically: GPUI
        // line-height multipliers + sub-pixel rounding eat ~20px more
        // than the formal `font_size * 1.5` math.
        let fudge = 20.0;
        match self.view {
            View::Main => {
                // pill row: py 10*2 + max(icon 32, body 13*1.5+11*1.5=36.75) ≈ 57
                let pills = 57.0 + 8.0 + 57.0;
                // slider row: py 6*2 + h 24 = 36
                // mt 10 + N brightness sliders (36 each) + gaps between
                // sliders (6 each) + the single volume slider (36).
                let n_bri = n_brightness.max(1) as f32;
                let sliders = 10.0 + n_bri * 36.0 + n_bri * 6.0 + 36.0;
                // nav row: py 10*2 + body 13*1.5+12*1.5=37.5 ≈ 58
                let rows = 8.0 + 58.0 + 2.0 + 58.0;
                // battery: py 10*2 + max(icon 28, text 14*1.5=21) = 48
                let battery = if has_battery { 8.0 + 48.0 } else { 0.0 };
                // footer = mt 2 + border 1 + pt 10 + button 36
                let footer = 2.0 + 1.0 + 10.0 + 36.0;
                padding + pills + sliders + rows + battery + footer + fudge
            }
            View::Wifi => {
                // header 32 + pb 12 + border 1
                let header = 45.0;
                // section label: text 10*1.5 + pt 12 + pb 4 ≈ 31
                let label = 31.0;
                // net row: py 8 + body 13*1.5 = 35
                let net_row = 36.0;
                let footer_link = 10.0 + 42.0;
                padding + header + label + net_row + label + net_row * 6.0 + footer_link + fudge
            }
            View::Bluetooth => {
                let header = 45.0;
                let label = 31.0;
                // device row: py 8 + body (13*1.5 + 11*1.5) ≈ 44
                let device_row = 44.0;
                let footer_link = 10.0 + 42.0;
                padding
                    + header
                    + label
                    + device_row * 4.0
                    + label
                    + device_row * 2.0
                    + footer_link
                    + fudge
            }
        }
    }
}

impl Focusable for QuickSettingsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for QuickSettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.global::<Theme>();
        self.ensure_scan_for_view(cx);
        let inner = match self.view {
            View::Main => self.render_main(cx).into_any_element(),
            View::Wifi => self.render_wifi(cx).into_any_element(),
            View::Bluetooth => self.render_bluetooth(cx).into_any_element(),
        };

        // Adaptive height: each view has different content; resize the
        // layer surface so its bottom hugs the content.
        let has_battery = self.modules.battery.read(cx).capacity().is_some();
        let n_brightness = self.modules.brightness.read(cx).displays().len().max(1);
        let desired = self.desired_height(has_battery, n_brightness);
        let current_h = f32::from(window.viewport_size().height);
        if (current_h - desired).abs() > 0.5 {
            window.resize(Size::new(px(PANEL_W), px(desired)));
        }

        // ztheme's `t.bg` carries alpha < 1 so the bar can be a translucent
        // top strip. For a *popup* we want a fully opaque surface so
        // toplevel windows behind it don't bleed through.
        let mut panel_bg = t.bg;
        panel_bg.a = 1.0;
        // Slider drag wiring: while the user holds the mouse down on a
        // track, mouse_move events on the panel root update the value;
        // mouse_up clears the drag.
        let bri_entity_drag = self.modules.brightness.clone();
        let drag = self.drag;
        let drag_weak_move = cx.entity().downgrade();
        let on_drag_move = move |ev: &gpui::MouseMoveEvent, _: &mut Window, cx: &mut App| {
            let Some(kind) = drag else { return };
            let frac = ((f32::from(ev.position.x) - SLIDER_TRACK_LEFT_PX) / SLIDER_TRACK_W_PX)
                .clamp(0.0, 1.0);
            let new_pct = (frac * 100.0).round() as u32;
            match kind {
                SliderKind::Brightness(target) => {
                    bri_entity_drag.read(cx).set_percent(target, new_pct);
                }
                SliderKind::Volume => {
                    VolumeModule::set_percent(new_pct);
                }
            }
            // Update the drag cache so the thumb tracks the cursor in
            // real time (system read-back is way too slow on DDC/CI).
            let _ = drag_weak_move.update(cx, |this: &mut Self, cx| {
                match kind {
                    SliderKind::Brightness(target) => {
                        this.drag_brightness.insert(target, new_pct);
                        if let Some(id) = target {
                            let label = this
                                .modules
                                .brightness
                                .read(cx)
                                .label_for(id)
                                .unwrap_or_default();
                            this.show_osd(id, &label, new_pct, cx);
                        }
                    }
                    SliderKind::Volume => this.drag_volume = Some(new_pct),
                }
                cx.notify();
            });
        };
        let drag_weak = cx.entity().downgrade();
        let on_drag_up = move |_: &gpui::MouseUpEvent, _: &mut Window, cx: &mut App| {
            // On release, keep the drag-cached value briefly so the thumb
            // doesn't snap back if the module poll hasn't caught up yet.
            let drag_weak2 = drag_weak.clone();
            let _ = drag_weak.update(cx, |this: &mut Self, _| {
                this.drag = None;
            });
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(800))
                    .await;
                let _ = drag_weak2.update(cx, |this: &mut Self, cx| {
                    this.drag_brightness.clear();
                    this.drag_volume = None;
                    this.close_all_osd(cx);
                    cx.notify();
                });
            })
            .detach();
        };

        div()
            .key_context("QuickSettings")
            .track_focus(&self.focus_handle(cx))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::back))
            .size_full()
            .bg(panel_bg)
            .text_color(t.fg)
            .text_size(theme::FONT_SIZE)
            .border_1()
            .border_color(t.border)
            .rounded_md()
            .overflow_hidden()
            .on_mouse_move(on_drag_move)
            .on_mouse_up(MouseButton::Left, on_drag_up)
            .child(div().p(px(14.)).child(inner))
    }
}

impl QuickSettingsPanel {
    fn render_main(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.global::<Theme>();
        // Snapshot every brightness target (one entry per DDC display,
        // or a single backlight entry).
        let brightness_rows: Vec<(Option<DisplayId>, String, u32)> = {
            let bri = self.modules.brightness.read(cx);
            let displays = bri.displays();
            if displays.is_empty() {
                vec![(
                    None,
                    "屏幕亮度".to_string(),
                    bri.percent().unwrap_or(0),
                )]
            } else {
                displays
                    .iter()
                    .map(|d| {
                        let label = if d.model == d.connector {
                            d.connector.clone()
                        } else {
                            format!("{} ({})", d.model, d.connector)
                        };
                        let pct = d
                            .display_id
                            .and_then(|id| bri.percent_for(id))
                            .or_else(|| bri.percent())
                            .unwrap_or(0);
                        (d.display_id, label, pct)
                    })
                    .collect()
            }
        };
        let (vol_pct_raw, _vol_muted) = {
            let v = self.modules.volume.read(cx);
            (v.percent().unwrap_or(0), v.is_muted())
        };
        let vol_pct = vol_pct_raw.min(100);
        let (bat_cap, bat_status) = {
            let b = self.modules.battery.read(cx);
            (b.capacity(), b.status())
        };

        let pills = self.pills;

        let entity = cx.entity().clone();
        let pill_wifi = pill(
            cx,
            "󰖩",
            "Wi-Fi",
            if pills.wifi { "已连接" } else { "已关闭" },
            pills.wifi,
            {
                let entity = entity.clone();
                move |cx| {
                    entity.update(cx, |p, cx| {
                        p.pills.wifi = !p.pills.wifi;
                        nmcli_set_wifi(p.pills.wifi);
                        cx.notify();
                    });
                }
            },
        );
        let pill_bt = pill(
            cx,
            "󰂯",
            "蓝牙",
            if pills.bluetooth {
                "2 已连接"
            } else {
                "已关闭"
            },
            pills.bluetooth,
            {
                let entity = entity.clone();
                move |cx| {
                    entity.update(cx, |p, cx| {
                        p.pills.bluetooth = !p.pills.bluetooth;
                        rfkill_set("bluetooth", p.pills.bluetooth);
                        cx.notify();
                    });
                }
            },
        );
        let pill_air = pill(
            cx,
            "✈",
            "飞行模式",
            if pills.airplane { "已开启" } else { "关闭" },
            pills.airplane,
            {
                let entity = entity.clone();
                move |cx| {
                    entity.update(cx, |p, cx| {
                        p.pills.airplane = !p.pills.airplane;
                        rfkill_set_all(p.pills.airplane);
                        // Airplane mode forces wifi/bluetooth off.
                        if p.pills.airplane {
                            p.pills.wifi = false;
                            p.pills.bluetooth = false;
                        }
                        cx.notify();
                    });
                }
            },
        );
        let pill_night = pill(
            cx,
            "󰽣",
            "夜间模式",
            if pills.night_light {
                "已开启"
            } else {
                "23:00 自动"
            },
            pills.night_light,
            {
                let entity = entity.clone();
                move |cx| {
                    entity.update(cx, |p, cx| {
                        p.pills.night_light = !p.pills.night_light;
                        toggle_night_light(p.pills.night_light);
                        cx.notify();
                    });
                }
            },
        );

        let half = px((PANEL_W - 24. - 8.) / 2.);
        let pills_grid = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(div().w(half).child(pill_wifi))
                    .child(div().w(half).child(pill_bt)),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(div().w(half).child(pill_air))
                    .child(div().w(half).child(pill_night)),
            );

        // Per-display brightness slider; closure targets that monitor only.
        let bri_sliders: Vec<gpui::AnyElement> = brightness_rows
            .iter()
            .map(|(target, label, pct)| {
                let kind = SliderKind::Brightness(*target);
                let display = self.drag_brightness.get(target).copied().unwrap_or(*pct);
                let target = *target;
                let label = label.clone();
                let bri_entity = self.modules.brightness.clone();
                let weak_panel = cx.entity().downgrade();
                slider_row(cx, kind, "󰃟", display, move |new_pct, cx| {
                    bri_entity.read(cx).set_percent(target, new_pct);
                    let label = label.clone();
                    let _ = weak_panel.update(cx, |p: &mut Self, cx| {
                        p.drag_brightness.insert(target, new_pct);
                        if let Some(id) = target {
                            p.show_osd(id, &label, new_pct, cx);
                        }
                        cx.notify();
                    });
                })
            })
            .collect();
        // Volume slider stays single-target.
        let vol_display = self.drag_volume.unwrap_or(vol_pct);
        let weak_panel = cx.entity().downgrade();
        let vol_slider = slider_row(
            cx,
            SliderKind::Volume,
            "󰕾",
            vol_display,
            move |new_pct, cx| {
                VolumeModule::set_percent(new_pct);
                let _ = weak_panel.update(cx, |p: &mut Self, cx| {
                    p.drag_volume = Some(new_pct);
                    cx.notify();
                });
            },
        );

        // Rows: Wi-Fi, Bluetooth.
        let wifi_value = if pills.wifi {
            "eero-living-room · 已连接".to_string()
        } else {
            "已关闭".to_string()
        };
        let entity = cx.entity().clone();
        let wifi_row = nav_row(cx, "󰖩", "Wi-Fi", &wifi_value, {
            let entity = entity.clone();
            move |cx| {
                entity.update(cx, |p, cx| {
                    p.view = View::Wifi;
                    cx.notify();
                });
            }
        });
        let bt_row = nav_row(cx, "󰂯", "蓝牙", "AirPods Pro · Magic Trackpad", {
            let entity = entity.clone();
            move |cx| {
                entity.update(cx, |p, cx| {
                    p.view = View::Bluetooth;
                    cx.notify();
                });
            }
        });

        // Battery strip — only render when a battery is actually present.
        let battery_strip = bat_cap.map(|c| render_battery_strip(cx, c, bat_status));

        // Footer: top border, 6px gap, flex:1 main button + two 36x36 icon
        // buttons (lock + power).
        let footer = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .pt(px(10.))
            .mt(px(2.))
            .border_t_1()
            .border_color(t.border)
            .child(
                div()
                    .flex_1()
                    .px(px(12.))
                    .py(px(8.))
                    .rounded_md()
                    .bg(t.surface_hover)
                    .text_color(t.fg)
                    .text_size(px(13.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(t.bg))
                    .on_mouse_down(MouseButton::Left, |_, _, _| {
                        spawn_settings_app();
                    })
                    .child(div().text_size(px(15.)).text_color(t.fg_dim).child("\u{f0493}"))
                    .child("设置"),
            )
            .child(
                div()
                    .w(px(36.))
                    .h(px(36.))
                    .flex_shrink_0()
                    .rounded_md()
                    .bg(t.surface_hover)
                    .text_color(t.fg_dim)
                    .text_size(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(move |s| s.bg(t.bg).text_color(t.fg))
                    .on_mouse_down(MouseButton::Left, |_, _, _| {
                        spawn_lock_screen();
                    })
                    .child("\u{f033e}"),
            )
            .child(
                div()
                    .w(px(36.))
                    .h(px(36.))
                    .flex_shrink_0()
                    .rounded_md()
                    .bg(t.surface_hover)
                    .text_color(t.fg_dim)
                    .text_size(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(move |s| s.text_color(t.urgent))
                    .on_mouse_down(MouseButton::Left, |_, _, _| {
                        spawn_power_menu();
                    })
                    .child("\u{f0425}"),
            );

        // Spacing follows the design's per-block margin-top.
        div()
            .flex()
            .flex_col()
            .child(pills_grid)
            .child(
                div()
                    .mt(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .children(bri_sliders)
                    .child(vol_slider),
            )
            .child(
                div()
                    .mt(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(wifi_row)
                    .child(bt_row),
            )
            .children(battery_strip.map(|s| div().mt(px(8.)).child(s)))
            .child(footer)
    }

    fn render_wifi(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.global::<Theme>();
        let entity = cx.entity().clone();

        let header = secondary_header(cx, "Wi-Fi", self.pills.wifi, {
            let entity = entity.clone();
            move |cx| {
                entity.update(cx, |p, cx| {
                    p.pills.wifi = !p.pills.wifi;
                    nmcli_set_wifi(p.pills.wifi);
                    cx.notify();
                });
            }
        });

        // If no Wi-Fi interface or no scan tool is available, show a single
        // placeholder row explaining that.
        let body: gpui::AnyElement = match self.scans.wifi_available {
            Some(false) => div()
                .flex()
                .flex_col()
                .gap_1()
                .child(placeholder_row(cx, "无可用的 Wi-Fi 接口"))
                .into_any_element(),
            _ => {
                let (connected, nearby): (Vec<_>, Vec<_>) = self
                    .scans
                    .wifi_networks
                    .iter()
                    .cloned()
                    .partition(|n| n.connected);
                let conn_rows: Vec<gpui::AnyElement> = if connected.is_empty() {
                    vec![placeholder_row(cx, "未连接")]
                } else {
                    connected
                        .iter()
                        .map(|n| {
                            wifi_row_view(cx, &n.ssid, signal_bars(n.signal), n.secured, true)
                        })
                        .collect()
                };
                let nearby_rows: Vec<gpui::AnyElement> = if nearby.is_empty() {
                    let msg = if self.scans.wifi_scanned && self.scans.wifi_networks.is_empty() {
                        "扫描中…"
                    } else {
                        "附近无网络"
                    };
                    vec![placeholder_row(cx, msg)]
                } else {
                    nearby
                        .iter()
                        .map(|n| {
                            wifi_row_view(cx, &n.ssid, signal_bars(n.signal), n.secured, false)
                        })
                        .collect()
                };
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(section_label(cx, "已连接"))
                    .child(div().flex().flex_col().gap_0p5().children(conn_rows))
                    .child(section_label(cx, "附近网络"))
                    .child(div().flex().flex_col().gap_0p5().children(nearby_rows))
                    .into_any_element()
            }
        };

        let footer_link = div()
            .mt_2()
            .p_2()
            .rounded_md()
            .border_1()
            .border_dashed()
            .border_color(t.border)
            .text_color(t.fg_dim)
            .flex()
            .items_center()
            .justify_center()
            .gap_1()
            .cursor_pointer()
            .hover(move |s| s.border_color(t.accent).text_color(t.accent))
            .on_mouse_down(MouseButton::Left, |_, _, _| {
                let _ = std::process::Command::new("nm-connection-editor")
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
            })
            .child("所有 Wi-Fi 设置  ›");

        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(header)
            .child(body)
            .child(footer_link)
    }

    fn render_bluetooth(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.global::<Theme>();
        let entity = cx.entity().clone();

        let header = secondary_header(cx, "蓝牙", self.pills.bluetooth, {
            let entity = entity.clone();
            move |cx| {
                entity.update(cx, |p, cx| {
                    p.pills.bluetooth = !p.pills.bluetooth;
                    rfkill_set("bluetooth", p.pills.bluetooth);
                    cx.notify();
                });
            }
        });

        let (paired_devices, nearby_devices): (Vec<_>, Vec<_>) =
            self.scans.bt_devices.iter().cloned().partition(|d| d.paired);

        let paired_rows: Vec<gpui::AnyElement> = if paired_devices.is_empty() {
            vec![placeholder_row(cx, if self.scans.bt_scanned { "扫描中…" } else { "无已配对设备" })]
        } else {
            paired_devices
                .iter()
                .map(|d| {
                    let sub = if d.pending {
                        // After the optimistic flip, `d.connected` is the
                        // *target* state, so true == we're moving toward
                        // "connected" == "连接中…".
                        if d.connected { "连接中…" } else { "断开中…" }
                    } else if d.unavailable {
                        "不可用 · 点击重试"
                    } else if d.connected {
                        "已连接"
                    } else {
                        "已配对"
                    };
                    let chip = if d.pending {
                        Some("…")
                    } else if d.connected {
                        Some("活动")
                    } else {
                        None
                    };
                    let path = d.path.clone();
                    let was_connected = d.connected;
                    let was_pending = d.pending;
                    let weak = cx.entity().downgrade();
                    let client = self.bt_client.clone();
                    let handler: BtRowClick = Box::new(move |cx: &mut App| {
                        if was_pending {
                            return;
                        }
                        let path = path.clone();
                        // Optimistic UI: flip apparent state + clear
                        // `unavailable` so a retry shows "连接中…".
                        let _ = weak.update(cx, |this: &mut Self, cx| {
                            if let Some(d) =
                                this.scans.bt_devices.iter_mut().find(|d| d.path == path)
                            {
                                d.connected = !was_connected;
                                d.pending = true;
                                d.unavailable = false;
                                cx.notify();
                            }
                        });
                        if was_connected {
                            client.disconnect_device(path);
                        } else {
                            client.connect_device(path);
                        }
                    });
                    bt_row_view(cx, bt_icon(&d.icon), &d.name, sub, None, chip, Some(handler))
                })
                .collect()
        };
        let nearby_rows: Vec<gpui::AnyElement> = if nearby_devices.is_empty() {
            vec![placeholder_row(cx, "附近无可配对设备")]
        } else {
            nearby_devices
                .iter()
                .map(|d| bt_row_view(cx, bt_icon(&d.icon), &d.name, "可配对", None, None, None))
                .collect()
        };

        let footer_link = div()
            .mt_2()
            .p_2()
            .rounded_md()
            .border_1()
            .border_dashed()
            .border_color(t.border)
            .text_color(t.fg_dim)
            .flex()
            .items_center()
            .justify_center()
            .gap_1()
            .cursor_pointer()
            .hover(move |s| s.border_color(t.accent).text_color(t.accent))
            .on_mouse_down(MouseButton::Left, |_, _, _| {
                let _ = std::process::Command::new("blueman-manager")
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
            })
            .child("所有蓝牙设置  ›");

        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(header)
            .child(section_label(cx, "我的设备"))
            .child(div().flex().flex_col().gap_0p5().children(paired_rows))
            .child(section_label(cx, "附近设备"))
            .child(div().flex().flex_col().gap_0p5().children(nearby_rows))
            .child(footer_link)
    }

    /// Open or refresh the brightness OSD on the given display. The
    /// auto-hide timer is **not** scheduled here — every mouse_move
    /// during a drag would spawn its own 700ms task and they'd all be
    /// outstanding at once. Instead, the panel's mouse_up handler calls
    /// `close_all_osd` after the 800ms drag-cache delay.
    pub(super) fn show_osd(
        &mut self,
        display_id: DisplayId,
        label: &str,
        pct: u32,
        cx: &mut Context<Self>,
    ) {
        if let Some(handle) = self.osd_windows.get_mut(&display_id) {
            let label = label.to_string();
            let _ = handle.window.update(cx, |osd, _w, cx| {
                osd.pct = pct;
                osd.label = label;
                cx.notify();
            });
            return;
        }
        let opts = WindowOptions {
            titlebar: None,
            window_bounds: Some(WindowBounds::Windowed(Bounds::maximized(
                Some(display_id),
                cx,
            ))),
            display_id: Some(display_id),
            app_id: Some("zbar-brightness-osd".to_string()),
            window_background: WindowBackgroundAppearance::Transparent,
            kind: WindowKind::LayerShell(LayerShellOptions {
                namespace: "zbar-brightness-osd".to_string(),
                layer: Layer::Overlay,
                anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
                margin: None,
                keyboard_interactivity: KeyboardInteractivity::None,
                exclusive_zone: Some(px(-1.0)),
                ..Default::default()
            }),
            ..Default::default()
        };
        let label_owned = label.to_string();
        let result = cx.open_window(opts, move |_window, cx| {
            cx.new(|cx| BrightnessOsd::new(pct, label_owned, cx))
        });
        match result {
            Ok(window) => {
                self.osd_windows.insert(display_id, OsdHandle { window });
            }
            Err(e) => {
                tracing::warn!("brightness OSD: open on {display_id:?} failed: {e}");
            }
        }
    }

    /// Tear down every brightness OSD window. Called when the panel
    /// itself is dismissed or when the user releases a drag.
    pub(super) fn close_all_osd(&mut self, cx: &mut App) {
        for (_, handle) in self.osd_windows.drain() {
            let _ = handle.window.update(cx, |_, window, _| window.remove_window());
        }
    }
}
