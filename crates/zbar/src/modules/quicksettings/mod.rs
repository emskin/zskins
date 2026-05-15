//! Ubuntu-style Quick Settings cluster + popup panel.
//!
//! Bar cluster shows: wifi/volume/battery icons + clock + caret. Clicking it
//! opens a layer-shell popup (the panel) with toggle pills, sliders,
//! expandable Wi-Fi/Bluetooth rows, battery strip, and a footer of action
//! buttons.
//!
//! Brightness/volume sliders drive [`BrightnessModule::set_percent`] and
//! [`VolumeModule::set_percent`] respectively; battery and network info
//! read live values from their existing entities.

mod bt;
mod osd;
mod panel;
mod system;
mod widgets;
mod wifi;

use crate::modules::battery::{BatteryModule, BatteryStatus};
use crate::modules::brightness::BrightnessModule;
use crate::modules::network::NetworkModule;
use crate::modules::volume::VolumeModule;
use crate::popup_catcher::{open_catchers_for, register_entity, PopupCatcher, PopupKind};
use chrono::{Datelike, Local, Timelike};
use gpui::{
    actions, div, layer_shell::*, point, prelude::*, px, AppContext, Bounds, Context, DisplayId,
    Entity, KeyBinding, MouseButton, Size, Window, WindowBackgroundAppearance, WindowBounds,
    WindowHandle, WindowKind, WindowOptions,
};
use panel::QuickSettingsPanel;
use std::time::Duration;
use system::{gammastep_running, nmcli_wifi_enabled, rfkill_all_blocked, rfkill_unblocked};
use ztheme::Theme;

actions!(zbar_quicksettings, [Dismiss, Back]);

pub fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("escape", Dismiss, Some("QuickSettings")),
        KeyBinding::new("backspace", Back, Some("QuickSettings")),
    ]
}

pub(super) const PANEL_W: f32 = 380.0;
/// Window-relative X of the slider track's left edge.
/// = panel padding (14) + icon column (22) + gap (10).
pub(super) const SLIDER_TRACK_LEFT_PX: f32 = 14.0 + 22.0 + 10.0;
/// Width of the slider track in pixels.
/// = PANEL_W - left padding (14) - icon (22) - gap (10) - value column (28) - gap (10) - right padding (14).
pub(super) const SLIDER_TRACK_W_PX: f32 = PANEL_W - 14.0 - 22.0 - 10.0 - 28.0 - 10.0 - 14.0;
/// Conservative initial layer-surface height. The panel calls
/// `window.resize()` once its content height is known on the first render,
/// so the layer surface shrinks to fit. Setting this too small clips the
/// first frame; too big leaves a flash of dead space.
const PANEL_H_INITIAL: f32 = 520.0;

#[derive(Clone)]
pub(super) struct Modules {
    pub(super) volume: Entity<VolumeModule>,
    pub(super) brightness: Entity<BrightnessModule>,
    pub(super) battery: Entity<BatteryModule>,
    pub(super) network: Entity<NetworkModule>,
}

#[derive(Clone, Copy)]
pub(super) struct PillsState {
    pub(super) wifi: bool,
    pub(super) bluetooth: bool,
    pub(super) airplane: bool,
    pub(super) night_light: bool,
}

impl PillsState {
    /// Probe the system at panel-open time so the pills reflect reality
    /// rather than always defaulting to "on".
    pub(super) fn read_from_system() -> Self {
        Self {
            wifi: nmcli_wifi_enabled().unwrap_or(true),
            bluetooth: rfkill_unblocked("bluetooth").unwrap_or(true),
            airplane: rfkill_all_blocked().unwrap_or(false),
            night_light: gammastep_running(),
        }
    }

    pub(super) fn matches(&self, other: &Self) -> bool {
        self.wifi == other.wifi
            && self.bluetooth == other.bluetooth
            && self.airplane == other.airplane
            && self.night_light == other.night_light
    }
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
