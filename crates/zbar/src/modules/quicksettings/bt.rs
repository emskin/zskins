//! Bluetooth UI types + row renderer for the secondary view.
//!
//! The DBus-level BlueZ client lives in `crate::bluetooth`; this module is
//! purely the panel-side representation (optimistic flags + icon mapping).

use super::panel::QuickSettingsPanel;
use gpui::{div, prelude::*, px, App, Context, MouseButton};
use ztheme::Theme;

#[derive(Clone, Debug)]
pub(super) struct BtDevice {
    /// BlueZ object path (e.g. `/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF`).
    /// Used as a stable handle for connect/disconnect requests.
    pub(super) path: String,
    pub(super) name: String,
    pub(super) icon: String,
    pub(super) connected: bool,
    pub(super) paired: bool,
    /// Optimistic UI: set true on click, cleared once the next BlueZ
    /// snapshot arrives. Renders as "连接中… / 断开中…".
    pub(super) pending: bool,
    /// Last connect attempt didn't take (paired but unreachable / off).
    /// Cleared on next user-initiated click.
    pub(super) unavailable: bool,
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

/// Map BlueZ `Device.Icon` hints to Nerd Font glyphs.
pub(super) fn bt_icon(icon: &str) -> &'static str {
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

pub(super) type BtRowClick = Box<dyn Fn(&mut App) + 'static>;

pub(super) fn bt_row_view(
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

#[cfg(test)]
mod tests {
    use super::bt_icon;

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
}
