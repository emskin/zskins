//! Ubuntu-style Quick Settings cluster + popup panel.
//!
//! Bar cluster shows: wifi/volume/battery icons + clock + caret. Clicking it
//! opens a layer-shell popup (the panel) with toggle pills, sliders, expandable
//! Wi-Fi/Bluetooth rows, battery strip, and a footer of action buttons.
//!
//! Brightness/volume sliders drive [`BrightnessModule::set_percent`] and
//! [`VolumeModule::set_percent`] respectively; battery, network info, etc.
//! read live values from their existing entities.
//!
//! Wi-Fi and Bluetooth secondary views currently use placeholder data — the
//! NetworkManager / BlueZ DBus integrations are intentionally deferred.

use crate::modules::battery::{BatteryModule, BatteryStatus};
use crate::modules::brightness::BrightnessModule;
use crate::modules::network::NetworkModule;
use crate::modules::volume::VolumeModule;
use crate::popup_catcher::{open_catchers_for, register_entity, PopupCatcher, PopupKind};
use crate::theme;
use chrono::{Datelike, Local, Timelike};
use gpui::{
    actions, div, layer_shell::*, point, prelude::*, px, App, AppContext, Bounds, Context,
    DisplayId, Entity, FocusHandle, Focusable, KeyBinding, MouseButton, Size, Window,
    WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions,
};
use std::time::Duration;
use ztheme::Theme;

actions!(zbar_quicksettings, [Dismiss, Back]);

pub fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("escape", Dismiss, Some("QuickSettings")),
        KeyBinding::new("backspace", Back, Some("QuickSettings")),
    ]
}

const PANEL_W: f32 = 380.0;
/// Window-relative X of the slider track's left edge.
/// = panel padding (14) + icon column (22) + gap (10).
const SLIDER_TRACK_LEFT_PX: f32 = 14.0 + 22.0 + 10.0;
/// Width of the slider track in pixels.
/// = PANEL_W - left padding (14) - icon (22) - gap (10) - value column (28) - gap (10) - right padding (14).
const SLIDER_TRACK_W_PX: f32 = PANEL_W - 14.0 - 22.0 - 10.0 - 28.0 - 10.0 - 14.0;
/// Conservative initial layer-surface height. The panel calls
/// `window.resize()` once its content height is known on the first render,
/// so the layer surface shrinks to fit. Setting this too small clips the
/// first frame; too big leaves a flash of dead space.
const PANEL_H_INITIAL: f32 = 520.0;

#[derive(Clone)]
struct Modules {
    volume: Entity<VolumeModule>,
    brightness: Entity<BrightnessModule>,
    battery: Entity<BatteryModule>,
    network: Entity<NetworkModule>,
}

pub struct QuickSettingsModule {
    display_id: Option<DisplayId>,
    modules: Modules,
    open: Option<OpenPanel>,
    close_tx: async_channel::Sender<()>,
    popup_kind: PopupKind,
}

struct OpenPanel {
    panel: WindowHandle<QuickSettingsPanel>,
    catchers: Vec<WindowHandle<PopupCatcher>>,
}

impl QuickSettingsModule {
    pub fn new(
        display_id: Option<DisplayId>,
        volume: Entity<VolumeModule>,
        brightness: Entity<BrightnessModule>,
        battery: Entity<BatteryModule>,
        network: Entity<NetworkModule>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe_global::<Theme>(|_, cx| cx.notify()).detach();

        // Re-render once a minute so the clock in the cluster stays current.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(30)).await;
            if this.update(cx, |_, cx| cx.notify()).is_err() {
                break;
            }
        })
        .detach();

        // Re-render whenever any sub-module changes so the cluster icons
        // (and the panel, if open) stay live.
        cx.observe(&volume, |_, _, cx| cx.notify()).detach();
        cx.observe(&brightness, |_, _, cx| cx.notify()).detach();
        cx.observe(&battery, |_, _, cx| cx.notify()).detach();
        cx.observe(&network, |_, _, cx| cx.notify()).detach();

        let (popup_kind, close_tx) = register_entity(cx, |m: &mut Self, cx| m.close(cx));

        Self {
            display_id,
            modules: Modules {
                volume,
                brightness,
                battery,
                network,
            },
            open: None,
            close_tx,
            popup_kind,
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if let Some(open) = self.open.take() {
            let _ = open.panel.update(cx, |panel, window, cx_panel| {
                panel.close_all_osd(cx_panel);
                window.remove_window();
            });
            crate::popup_catcher::close_catchers(cx, open.catchers);
            cx.notify();
        }
    }

    fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.open.is_some() {
            self.close(cx);
            return;
        }
        crate::popup_catcher::dismiss_others(cx, self.popup_kind);

        let opts = WindowOptions {
            titlebar: None,
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: Size::new(px(PANEL_W), px(PANEL_H_INITIAL)),
            })),
            display_id: self.display_id,
            app_id: Some("zbar-quicksettings".to_string()),
            window_background: WindowBackgroundAppearance::Opaque,
            kind: WindowKind::LayerShell(LayerShellOptions {
                namespace: "zbar-quicksettings".to_string(),
                layer: Layer::Overlay,
                anchor: Anchor::TOP | Anchor::RIGHT,
                margin: Some((px(0.), px(8.), px(0.), px(0.))),
                keyboard_interactivity: KeyboardInteractivity::OnDemand,
                exclusive_zone: None,
                ..Default::default()
            }),
            ..Default::default()
        };

        let catchers =
            open_catchers_for(cx, "zbar-quicksettings-catcher", self.close_tx.clone());
        let modules = self.modules.clone();
        match cx.open_window(opts, |_, cx| {
            cx.new(|cx| QuickSettingsPanel::new(modules, cx))
        }) {
            Ok(panel) => {
                self.open = Some(OpenPanel { panel, catchers });
                cx.notify();
            }
            Err(e) => {
                tracing::warn!("quicksettings: failed to open panel: {e}");
                crate::popup_catcher::close_catchers(cx, catchers);
            }
        }
    }
}

fn fmt_clock_short(d: &chrono::DateTime<Local>) -> String {
    let wd = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];
    format!(
        "{} {:02}:{:02}",
        wd[d.weekday().num_days_from_sunday() as usize],
        d.hour(),
        d.minute()
    )
}

impl Render for QuickSettingsModule {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.global::<Theme>();
        let entity = cx.entity().clone();

        let vol = self.modules.volume.read(cx);
        let bat = self.modules.battery.read(cx);
        let wifi_on = self
            .modules
            .network
            .read(cx)
            .interfaces()
            .iter()
            .any(|i| i.is_wireless && i.operstate == crate::net_info::OperState::Up);

        let wifi_icon = if wifi_on { "󰖩" } else { "󰖪" };
        let vol_icon = match (vol.is_muted(), vol.percent().unwrap_or(0)) {
            (true, _) => "󰝟",
            (_, 0) => "󰕿",
            (_, 1..=50) => "󰖀",
            _ => "󰕾",
        };
        let bat_icon = match (bat.status(), bat.capacity().unwrap_or(0)) {
            (BatteryStatus::Charging, _) => "󰂄",
            (_, 0..=10) => "󰁺",
            (_, 11..=30) => "󰁼",
            (_, 31..=60) => "󰁾",
            (_, 61..=90) => "󰂀",
            _ => "󰁹",
        };
        let clock = fmt_clock_short(&Local::now());

        div()
            .id("zbar-quicksettings-cluster")
            .h(px(28.))
            .flex()
            .items_center()
            .gap(px(10.))
            .pl(px(12.))
            .pr(px(4.))
            .rounded_full()
            .bg(t.surface)
            .border_1()
            .border_color(t.border)
            .text_size(px(12.))
            .text_color(t.fg_dim)
            .cursor_pointer()
            .hover(move |s| s.bg(t.surface_hover))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_color(t.fg_dim)
                    .child(wifi_icon.to_string())
                    .child(vol_icon.to_string())
                    .child(bat_icon.to_string()),
            )
            .child(div().text_color(t.fg).child(clock))
            .child(
                div()
                    .w(px(20.))
                    .h(px(20.))
                    .rounded_full()
                    .bg(t.accent_soft)
                    .text_color(t.accent)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child("▾"),
            )
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                entity.update(cx, |m, cx| m.toggle(cx));
            })
    }
}

// ===========================================================================
// Panel popup
// ===========================================================================

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Main,
    Wifi,
    Bluetooth,
}

#[derive(Clone, Copy)]
struct PillsState {
    wifi: bool,
    bluetooth: bool,
    airplane: bool,
    night_light: bool,
}

impl PillsState {
    /// Probe the system at panel-open time so the pills reflect reality
    /// rather than always defaulting to "on".
    fn read_from_system() -> Self {
        Self {
            wifi: nmcli_wifi_enabled().unwrap_or(true),
            bluetooth: rfkill_unblocked("bluetooth").unwrap_or(true),
            airplane: rfkill_all_blocked().unwrap_or(false),
            night_light: gammastep_running(),
        }
    }

    fn matches(&self, other: &Self) -> bool {
        self.wifi == other.wifi
            && self.bluetooth == other.bluetooth
            && self.airplane == other.airplane
            && self.night_light == other.night_light
    }
}

#[derive(Clone, Debug)]
struct BtDevice {
    /// BlueZ object path (e.g. `/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF`).
    /// Used as a stable handle for connect/disconnect requests.
    path: String,
    name: String,
    icon: String,
    connected: bool,
    paired: bool,
    /// Optimistic UI: set true on click, cleared once the next BlueZ
    /// snapshot arrives. Renders as "连接中… / 断开中…".
    pending: bool,
    /// Last connect attempt didn't take (paired but unreachable / off).
    /// Cleared on next user-initiated click.
    unavailable: bool,
}

impl From<crate::bluetooth::BtDevice> for BtDevice {
    fn from(d: crate::bluetooth::BtDevice) -> Self {
        Self {
            path: d.path,
            name: d.name,
            icon: d.icon,
            connected: d.connected,
            paired: d.paired,
            pending: false,
            unavailable: false,
        }
    }
}

#[derive(Clone, Debug)]
struct WifiEntry {
    ssid: String,
    signal: u8,
    secured: bool,
    connected: bool,
}

#[derive(Default)]
struct ScanState {
    bt_devices: Vec<BtDevice>,
    bt_scanned: bool,
    wifi_networks: Vec<WifiEntry>,
    wifi_scanned: bool,
    wifi_available: Option<bool>,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
enum SliderKind {
    /// `None` = laptop backlight / single-display fallback.
    /// `Some(id)` = one of N DDC/CI monitors (one slider per display).
    Brightness(Option<DisplayId>),
    Volume,
}

pub struct QuickSettingsPanel {
    modules: Modules,
    view: View,
    pills: PillsState,
    focus_handle: FocusHandle,
    scans: ScanState,
    /// Which slider (if any) is currently being dragged. Set by mouse_down
    /// on the track, used by mouse_move on the panel root, cleared by
    /// mouse_up on the panel root.
    drag: Option<SliderKind>,
    /// Optimistic slider values shown while the user is dragging. Backing
    /// modules only re-read every few seconds, so without this the thumb
    /// would appear frozen during a drag. Keyed by display so each
    /// per-monitor brightness slider has its own latch.
    drag_brightness: std::collections::HashMap<Option<DisplayId>, u32>,
    drag_volume: Option<u32>,
    bt_client: crate::bluetooth::BluetoothClient,
    /// Per-display brightness OSD overlays. Keyed by `DisplayId`; each
    /// entry is the live window plus its current state (label + pct +
    /// the close-timer generation counter used to ignore stale timers).
    osd_windows: std::collections::HashMap<DisplayId, OsdHandle>,
}

struct OsdHandle {
    window: gpui::WindowHandle<BrightnessOsd>,
}

impl QuickSettingsPanel {
    fn new(modules: Modules, cx: &mut Context<Self>) -> Self {
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
        // layer surface so its bottom hugs the content. We compute the
        // ideal height per view and call `window.resize()` when the
        // current viewport size is off by more than half a pixel.
        let has_battery = self.modules.battery.read(cx).capacity().is_some();
        let n_brightness = self.modules.brightness.read(cx).displays().len().max(1);
        let desired = self.desired_height(has_battery, n_brightness);
        let current_h = f32::from(window.viewport_size().height);
        if (current_h - desired).abs() > 0.5 {
            window.resize(Size::new(px(PANEL_W), px(desired)));
        }

        // ztheme's `t.bg` carries alpha < 1 so the bar can be a translucent
        // top strip. For a *popup* we want a fully opaque surface so
        // toplevel windows behind it don't bleed through. Force alpha=1
        // while keeping hue/saturation/lightness in lockstep with the
        // current theme.
        let mut panel_bg = t.bg;
        panel_bg.a = 1.0;
        // Slider drag wiring: while the user holds the mouse down on a
        // track, mouse_move events on the panel root update the value;
        // mouse_up clears the drag. This works even if the cursor briefly
        // strays off the track, as long as it stays inside the panel.
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
            // We schedule a short delay then clear the cache; by then the
            // 5-15s poll has typically already fetched the new value.
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
        // or a single backlight entry). Empty fallback shows a single
        // slider tied to whatever the module's primary value is.
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

        // Spacing follows the design's per-block margin-top:
        // sliders.mt = 10, rows.mt = 8, battery.mt = 8, actions.mt = 10.
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
        // placeholder row explaining that. Otherwise split scanned entries
        // by connected state.
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
                        // BluetoothClient pushes a fresh snapshot after the
                        // op completes; that overwrites our optimistic
                        // state + clears `pending`. Unavailable flagging
                        // happens in the snapshot consumer (`new`'s spawn).
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
}

// ===========================================================================
// Sub-components
// ===========================================================================

fn pill<F>(
    cx: &mut Context<QuickSettingsPanel>,
    icon: &str,
    title: &str,
    sub: &str,
    on: bool,
    on_click: F,
) -> gpui::AnyElement
where
    F: Fn(&mut App) + 'static,
{
    let t = *cx.global::<Theme>();
    let (bg, fg_title, fg_sub, icon_bg, icon_fg) = if on {
        (
            t.accent,
            gpui::white(),
            gpui::rgba(0xffffffc7).into(),
            gpui::rgba(0xffffff29).into(),
            gpui::white(),
        )
    } else {
        (t.surface_hover, t.fg, t.fg_dim, t.surface, t.fg_dim)
    };
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(12.))
        .py(px(10.))
        .rounded_md()
        .bg(bg)
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |_, _, cx| on_click(cx))
        .child(
            div()
                .w(px(32.))
                .h(px(32.))
                .flex_shrink_0()
                .rounded_full()
                .bg(icon_bg)
                .text_color(icon_fg)
                .text_size(px(16.))
                .flex()
                .items_center()
                .justify_center()
                .child(icon.to_string()),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .min_w(px(0.))
                .overflow_hidden()
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(fg_title)
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(fg_sub)
                        .child(sub.to_string()),
                ),
        )
        .into_any_element()
}

fn slider_row<F>(
    cx: &mut Context<QuickSettingsPanel>,
    kind: SliderKind,
    icon: &str,
    value: u32,
    on_change: F,
) -> gpui::AnyElement
where
    F: Fn(u32, &mut App) + 'static + Clone,
{
    let t = *cx.global::<Theme>();
    let pct = value.min(100);
    let fill_frac = pct as f32 / 100.0;
    let weak = cx.entity().downgrade();

    // Mouse-down on the track immediately commits the click position AND
    // arms drag state. Subsequent mouse_move events on the panel root
    // (see `attach_drag_handlers`) keep updating while held, mouse_up
    // there clears the drag.
    let on_change_md = on_change.clone();
    let click_handler = move |ev: &gpui::MouseDownEvent, _: &mut Window, cx: &mut App| {
        let frac = ((f32::from(ev.position.x) - SLIDER_TRACK_LEFT_PX) / SLIDER_TRACK_W_PX)
            .clamp(0.0, 1.0);
        let new_pct = (frac * 100.0).round() as u32;
        on_change_md(new_pct, cx);
        let _ = weak.update(cx, |this: &mut QuickSettingsPanel, _| {
            this.drag = Some(kind);
        });
    };

    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(8.))
        .py(px(6.))
        .rounded_md()
        .hover(move |s| s.bg(t.surface_hover))
        .child(
            div()
                .w(px(22.))
                .flex_shrink_0()
                .text_size(px(16.))
                .text_color(t.fg_dim)
                .child(icon.to_string()),
        )
        .child(
            div()
                .relative()
                .flex_1()
                .h(px(24.))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, click_handler)
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .top(px(10.))
                        .bottom(px(10.))
                        .rounded_full()
                        .bg(t.border),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(10.))
                        .bottom(px(10.))
                        .left(px(0.))
                        .w(gpui::relative(fill_frac.max(0.001)))
                        .rounded_full()
                        .bg(t.accent),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(0.))
                        .left(px(0.))
                        .h(px(24.))
                        .w(gpui::relative(fill_frac))
                        .flex()
                        .items_center()
                        .justify_end()
                        .child(
                            div()
                                .w(px(18.))
                                .h(px(18.))
                                .mr(px(-9.))
                                .rounded_full()
                                .bg(t.surface)
                                .border_1()
                                .border_color(t.border),
                        ),
                ),
        )
        .child(
            div()
                .min_w(px(28.))
                .flex_shrink_0()
                .flex()
                .justify_end()
                .text_color(t.fg_dim)
                .text_size(px(11.))
                .child(format!("{pct}")),
        )
        .into_any_element()
}

fn nav_row<F>(
    cx: &mut Context<QuickSettingsPanel>,
    icon: &str,
    label: &str,
    value: &str,
    on_click: F,
) -> gpui::AnyElement
where
    F: Fn(&mut App) + 'static,
{
    let t = *cx.global::<Theme>();
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .pl(px(8.))
        .pr(px(10.))
        .py(px(10.))
        .rounded_md()
        .cursor_pointer()
        .hover(move |s| s.bg(t.surface_hover))
        .on_mouse_down(MouseButton::Left, move |_, _, cx| on_click(cx))
        .child(
            div()
                .w(px(28.))
                .flex_shrink_0()
                .text_size(px(18.))
                .text_color(t.fg)
                .child(icon.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(t.fg)
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(t.fg_dim)
                        .child(value.to_string()),
                ),
        )
        .child(div().text_size(px(14.)).text_color(t.fg_dim).child("›"))
        .into_any_element()
}

fn render_battery_strip(
    cx: &mut Context<QuickSettingsPanel>,
    cap: u8,
    status: BatteryStatus,
) -> gpui::AnyElement {
    let t = *cx.global::<Theme>();
    let cap_str = format!("{cap}%");
    let (chip_text, icon_color) = match status {
        BatteryStatus::Charging => (Some("充电中"), t.success),
        BatteryStatus::Full => (Some("已充满"), t.success),
        BatteryStatus::Discharging => (None, t.fg),
        BatteryStatus::Unknown => (None, t.fg_dim),
    };
    let mut row = div()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(12.))
        .py(px(10.))
        .rounded_md()
        .bg(t.surface_hover)
        .child(
            div()
                .w(px(28.))
                .flex_shrink_0()
                .text_size(px(20.))
                .text_color(icon_color)
                .child("󰂄".to_string()),
        )
        .child(
            div()
                .flex_1()
                .flex()
                .items_baseline()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(14.))
                        .text_color(t.fg)
                        .child(cap_str),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(t.fg_dim)
                        .child("剩余约 4 小时 12 分".to_string()),
                ),
        );
    if let Some(label) = chip_text {
        row = row.child(
            div()
                .text_size(px(10.))
                .px(px(7.))
                .py(px(3.))
                .rounded_full()
                .bg(t.success)
                .text_color(gpui::white())
                .child(label.to_string()),
        );
    }
    row.into_any_element()
}

fn secondary_header<F>(
    cx: &mut Context<QuickSettingsPanel>,
    title: &str,
    enabled: bool,
    on_toggle: F,
) -> gpui::AnyElement
where
    F: Fn(&mut App) + 'static,
{
    let t = *cx.global::<Theme>();
    let entity = cx.entity().clone();
    div()
        .flex()
        .items_center()
        .gap_2()
        .pb_2()
        .border_b_1()
        .border_color(t.border)
        .child(
            div()
                .w(px(28.))
                .h(px(28.))
                .rounded_md()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_color(t.fg_dim)
                .hover(move |s| s.bg(t.surface_hover).text_color(t.fg))
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    entity.update(cx, |p, cx| {
                        p.view = View::Main;
                        cx.notify();
                    });
                })
                .child("‹"),
        )
        .child(div().flex_1().text_color(t.fg).child(title.to_string()))
        .child(simple_switch(cx, enabled, on_toggle))
        .into_any_element()
}

fn simple_switch<F>(
    cx: &mut Context<QuickSettingsPanel>,
    on: bool,
    on_toggle: F,
) -> gpui::AnyElement
where
    F: Fn(&mut App) + 'static,
{
    let t = *cx.global::<Theme>();
    let track_bg = if on { t.accent } else { t.border };
    let knob_left = if on { px(16.) } else { px(2.) };
    div()
        .relative()
        .w(px(34.))
        .h(px(20.))
        .rounded_full()
        .bg(track_bg)
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |_, _, cx| on_toggle(cx))
        .child(
            div()
                .absolute()
                .top(px(2.))
                .left(knob_left)
                .w(px(16.))
                .h(px(16.))
                .rounded_full()
                .bg(gpui::white()),
        )
        .into_any_element()
}

fn section_label(cx: &mut Context<QuickSettingsPanel>, text: &str) -> gpui::AnyElement {
    let t = *cx.global::<Theme>();
    div()
        .px_2()
        .pt_3()
        .pb_1()
        .text_size(px(10.))
        .text_color(t.fg_dim)
        .child(text.to_string().to_uppercase())
        .into_any_element()
}

fn wifi_row_view(
    cx: &mut Context<QuickSettingsPanel>,
    name: &str,
    signal: u8,
    locked: bool,
    connected: bool,
) -> gpui::AnyElement {
    let t = *cx.global::<Theme>();
    let signal_color = match signal {
        3 => t.fg,
        2 => t.fg_dim,
        _ => t.border,
    };
    div()
        .flex()
        .items_center()
        .gap_2p5()
        .px_2()
        .py_2()
        .rounded_md()
        .cursor_pointer()
        .hover(move |s| s.bg(t.surface_hover))
        .child(div().w(px(18.)).text_color(signal_color).child("󰖩".to_string()))
        .child(
            div()
                .flex_1()
                .text_color(t.fg)
                .child(name.to_string()),
        )
        .child(if locked {
            div()
                .w(px(14.))
                .text_color(t.fg_dim)
                .text_size(px(11.))
                .child("🔒".to_string())
                .into_any_element()
        } else {
            div().w(px(14.)).into_any_element()
        })
        .child(if connected {
            div()
                .w(px(18.))
                .text_color(t.accent)
                .child("✓".to_string())
                .into_any_element()
        } else {
            div().w(px(18.)).into_any_element()
        })
        .into_any_element()
}

type BtRowClick = Box<dyn Fn(&mut App) + 'static>;

fn bt_row_view(
    cx: &mut Context<QuickSettingsPanel>,
    icon: &str,
    name: &str,
    sub: &str,
    battery: Option<&str>,
    chip: Option<&str>,
    on_click: Option<BtRowClick>,
) -> gpui::AnyElement {
    let t = *cx.global::<Theme>();
    let mut row = div()
        .flex()
        .items_center()
        .gap_2p5()
        .px_2()
        .py_2()
        .rounded_md()
        .cursor_pointer()
        .hover(move |s| s.bg(t.surface_hover));
    if let Some(handler) = on_click {
        row = row.on_mouse_down(MouseButton::Left, move |_, _, cx| handler(cx));
    }
    let mut row = row
        .child(div().w(px(22.)).text_color(t.fg_dim).child(icon.to_string()))
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .child(div().text_color(t.fg).child(name.to_string()))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(t.fg_dim)
                        .child(sub.to_string()),
                ),
        );
    if let Some(b) = battery {
        row = row.child(
            div()
                .text_size(px(11.))
                .text_color(t.fg_dim)
                .child(b.to_string()),
        );
    }
    if let Some(c) = chip {
        row = row.child(
            div()
                .ml_2()
                .text_size(px(10.))
                .px_1p5()
                .py_0p5()
                .rounded_full()
                .bg(t.accent_soft)
                .text_color(t.accent)
                .child(c.to_string()),
        );
    }
    row.into_any_element()
}

fn placeholder_row(cx: &mut Context<QuickSettingsPanel>, text: &str) -> gpui::AnyElement {
    let t = *cx.global::<Theme>();
    div()
        .px(px(8.))
        .py(px(8.))
        .text_size(px(12.))
        .text_color(t.fg_dim)
        .child(text.to_string())
        .into_any_element()
}

/// Map BlueZ Device.Icon hints to Nerd Font glyphs.
fn bt_icon(icon: &str) -> &'static str {
    match icon {
        "audio-headphones" | "audio-headset" | "audio-earphone" => "\u{f025f}", // headphones
        "audio-card" | "audio-speakers" => "\u{f057e}",                          // speaker
        "input-keyboard" => "\u{f030c}",                                          // keyboard
        "input-mouse" => "\u{f037d}",                                             // mouse
        "input-gaming" => "\u{f0eb5}",                                            // gamepad
        "phone" | "phone-smart" => "\u{f011c}",                                   // cellphone
        "computer" | "computer-laptop" => "\u{f0379}",                            // laptop
        "video-display" | "tv" => "\u{f07f9}",                                    // television
        "camera-photo" | "camera" => "\u{f0100}",                                 // camera
        "watch" | "smartwatch" => "\u{f0fc9}",                                    // watch
        _ => "\u{f00af}",                                                          // generic bluetooth
    }
}

fn signal_bars(percent: u8) -> u8 {
    match percent {
        70..=100 => 3,
        40..=69 => 2,
        _ => 1,
    }
}

// `scan_bluetooth_devices` was the bluetoothctl-based polling impl.
// The BlueZ DBus client (see `crate::bluetooth`) replaces it: snapshots
// push directly to the panel via an async channel.

/// Returns `(wifi_available, networks)`. `wifi_available=false` means we
/// found no wifi interface or no tool that can list networks, which the
/// view renders as a dedicated "no Wi-Fi" placeholder.
fn scan_wifi_networks() -> (bool, Vec<WifiEntry>) {
    // No wireless interfaces under /sys/class/net? Bail out cheaply.
    let has_wifi = std::fs::read_dir("/sys/class/net")
        .map(|rd| {
            rd.flatten().any(|e| {
                let name = e.file_name();
                let n = name.to_string_lossy();
                std::path::Path::new(&format!("/sys/class/net/{n}/wireless")).exists()
            })
        })
        .unwrap_or(false);
    if !has_wifi {
        return (false, Vec::new());
    }
    // Try nmcli first; fall back to empty list. (Real iwd/wpa fallback
    // can be added later; this machine's path through `has_wifi=false`
    // means we never reach here today.)
    let out = match Command::new("nmcli")
        .args(["-t", "-f", "IN-USE,SSID,SIGNAL,SECURITY", "device", "wifi", "list"])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return (true, Vec::new()),
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut entries = Vec::new();
    for line in text.lines() {
        // Columns are colon-separated; SSID may itself contain escaped
        // colons (`\:`) — split with a small state machine.
        let cols = split_nmcli_columns(line);
        if cols.len() < 4 {
            continue;
        }
        let connected = cols[0].trim() == "*";
        let ssid = cols[1].trim().to_string();
        if ssid.is_empty() {
            continue;
        }
        let signal: u8 = cols[2].trim().parse().unwrap_or(0);
        let secured = !cols[3].trim().is_empty() && cols[3].trim() != "--";
        entries.push(WifiEntry {
            ssid,
            signal,
            secured,
            connected,
        });
    }
    entries.sort_by(|a, b| b.connected.cmp(&a.connected).then_with(|| b.signal.cmp(&a.signal)));
    (true, entries)
}

fn split_nmcli_columns(line: &str) -> Vec<String> {
    let mut cols = Vec::new();
    let mut buf = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&n) = chars.peek() {
                buf.push(n);
                chars.next();
            }
        } else if c == ':' {
            cols.push(std::mem::take(&mut buf));
        } else {
            buf.push(c);
        }
    }
    cols.push(buf);
    cols
}

// ===========================================================================
// System integration helpers
//
// All commands run via std::process::Command::spawn (non-blocking) and
// silently swallow errors — we don't want UI to block on slow tools and
// the next state read on the next panel open will reflect reality.
// ===========================================================================

use std::process::{Command, Stdio};

fn nmcli_wifi_enabled() -> Option<bool> {
    let out = Command::new("nmcli")
        .args(["-t", "radio", "wifi"])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    Some(s.trim() == "enabled")
}

fn nmcli_set_wifi(on: bool) {
    let _ = Command::new("nmcli")
        .args(["radio", "wifi", if on { "on" } else { "off" }])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn rfkill_unblocked(kind: &str) -> Option<bool> {
    // `rfkill list -no SOFT bluetooth` prints "unblocked" or "blocked".
    let out = Command::new("rfkill")
        .args(["list", "-no", "SOFT", kind])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    if s.trim().is_empty() {
        return None;
    }
    Some(s.contains("unblocked"))
}

fn rfkill_set(kind: &str, enabled: bool) {
    let _ = Command::new("rfkill")
        .args([if enabled { "unblock" } else { "block" }, kind])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn rfkill_all_blocked() -> Option<bool> {
    let out = Command::new("rfkill")
        .args(["list", "-no", "SOFT"])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return None;
    }
    Some(lines.iter().all(|l| l.contains("blocked") && !l.contains("unblocked")))
}

fn rfkill_set_all(airplane: bool) {
    let _ = Command::new("rfkill")
        .args([if airplane { "block" } else { "unblock" }, "all"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// Best-effort night-light detection — checks whether gammastep/wlsunset/
/// redshift is running. We don't try to introspect their state.
fn gammastep_running() -> bool {
    for name in ["gammastep", "wlsunset", "redshift"] {
        if Command::new("pgrep")
            .args(["-x", name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

/// Toggle the first available night-light helper. Starts in background on
/// enable, kills it on disable.
fn toggle_night_light(enable: bool) {
    if enable {
        for name in ["gammastep", "wlsunset", "redshift"] {
            if Command::new(name)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .is_ok()
            {
                return;
            }
        }
    } else {
        for name in ["gammastep", "wlsunset", "redshift"] {
            let _ = Command::new("pkill")
                .args(["-x", name])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
        }
    }
}

fn spawn_settings_app() {
    // Try a handful of common settings apps in order of preference.
    for cmd in [
        "gnome-control-center",
        "systemsettings",
        "pavucontrol",
        "xdg-open",
    ] {
        if Command::new(cmd)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok()
        {
            return;
        }
    }
}

fn spawn_lock_screen() {
    // Prefer per-WM lockers, fall back to loginctl.
    for (cmd, args) in [
        ("swaylock", &[][..]),
        ("hyprlock", &[][..]),
        ("loginctl", &["lock-session"][..]),
    ] {
        if Command::new(cmd)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok()
        {
            return;
        }
    }
}

fn spawn_power_menu() {
    // No standardized power-menu app; for now just trigger logout via
    // loginctl. A proper implementation would open a confirmation popup
    // with logout/restart/shutdown options.
    let _ = Command::new("loginctl")
        .args(["terminate-user", &std::env::var("USER").unwrap_or_default()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

// ===========================================================================
// Brightness OSD — screen-centered overlay shown on the display being
// adjusted via the brightness slider. Auto-dismisses ~700ms after the
// last update.
// ===========================================================================

const OSD_SIZE_PX: f32 = 160.0;

pub struct BrightnessOsd {
    pct: u32,
    label: String,
}

impl BrightnessOsd {
    fn new(pct: u32, label: String, cx: &mut Context<Self>) -> Self {
        cx.observe_global::<Theme>(|_, cx| cx.notify()).detach();
        Self { pct, label }
    }
}

impl Render for BrightnessOsd {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.global::<Theme>();
        let mut bg = t.bg;
        bg.a = 1.0;
        let pct = self.pct.min(100);
        let fill_frac = pct as f32 / 100.0;
        // Full-screen transparent root; a centered 160x160 card carries
        // the actual visuals.
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(OSD_SIZE_PX))
                    .h(px(OSD_SIZE_PX))
                    .rounded_lg()
                    .bg(bg)
                    .border_1()
                    .border_color(t.border)
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(48.))
                            .text_color(t.accent)
                            .child("\u{f00de}".to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(26.))
                            .text_color(t.fg)
                            .child(format!("{pct}%")),
                    )
                    .child(
                        div()
                            .relative()
                            .w(px(OSD_SIZE_PX - 40.0))
                            .h(px(4.))
                            .rounded_full()
                            .bg(t.border)
                            .child(
                                div()
                                    .absolute()
                                    .left(px(0.))
                                    .top(px(0.))
                                    .bottom(px(0.))
                                    .w(gpui::relative(fill_frac.max(0.001)))
                                    .rounded_full()
                                    .bg(t.accent),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(t.fg_dim)
                            .child(self.label.clone()),
                    ),
            )
    }
}

impl QuickSettingsPanel {
    /// Open or refresh the brightness OSD on the given display. The
    /// auto-hide timer is **not** scheduled here — every mouse_move
    /// during a drag would spawn its own 700ms task and they'd all be
    /// outstanding at once. Instead, the panel's mouse_up handler calls
    /// `close_all_osd` after the 800ms drag-cache delay.
    fn show_osd(&mut self, display_id: DisplayId, label: &str, pct: u32, cx: &mut Context<Self>) {
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
    fn close_all_osd(&mut self, cx: &mut App) {
        for (_, handle) in self.osd_windows.drain() {
            let _ = handle.window.update(cx, |_, window, _| window.remove_window());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{bt_icon, signal_bars, split_nmcli_columns};

    #[test]
    fn bt_icon_maps_known_classes() {
        assert_eq!(bt_icon("audio-headphones"), "\u{f025f}");
        assert_eq!(bt_icon("audio-headset"), "\u{f025f}");
        assert_eq!(bt_icon("audio-speakers"), "\u{f057e}");
        assert_eq!(bt_icon("input-keyboard"), "\u{f030c}");
        assert_eq!(bt_icon("input-mouse"), "\u{f037d}");
        assert_eq!(bt_icon("phone-smart"), "\u{f011c}");
    }

    #[test]
    fn bt_icon_falls_back_to_generic() {
        assert_eq!(bt_icon(""), "\u{f00af}");
        assert_eq!(bt_icon("nonexistent-class"), "\u{f00af}");
    }

    #[test]
    fn signal_bars_buckets_percent() {
        assert_eq!(signal_bars(0), 1);
        assert_eq!(signal_bars(39), 1);
        assert_eq!(signal_bars(40), 2);
        assert_eq!(signal_bars(69), 2);
        assert_eq!(signal_bars(70), 3);
        assert_eq!(signal_bars(100), 3);
    }

    #[test]
    fn split_nmcli_columns_basic() {
        let line = "*:MyNetwork:75:WPA2";
        let cols = split_nmcli_columns(line);
        assert_eq!(cols, vec!["*", "MyNetwork", "75", "WPA2"]);
    }

    #[test]
    fn split_nmcli_columns_handles_escaped_colon_in_ssid() {
        // nmcli -t escapes ':' inside SSIDs as `\:` — must not split there.
        let line = " :Café\\:Wifi:60:WPA2";
        let cols = split_nmcli_columns(line);
        assert_eq!(cols, vec![" ", "Café:Wifi", "60", "WPA2"]);
    }

    #[test]
    fn split_nmcli_columns_empty_fields_preserved() {
        let line = "::::";
        let cols = split_nmcli_columns(line);
        assert_eq!(cols, vec!["", "", "", "", ""]);
    }

    #[test]
    fn split_nmcli_columns_open_network_blank_security() {
        let line = " :OpenNet:50:";
        let cols = split_nmcli_columns(line);
        assert_eq!(cols, vec![" ", "OpenNet", "50", ""]);
    }
}
