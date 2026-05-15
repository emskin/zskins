//! Wi-Fi data + scan + row renderer for the secondary view.

use super::panel::QuickSettingsPanel;
use gpui::{div, prelude::*, px, Context};
use std::process::Command;
use ztheme::Theme;

#[derive(Clone, Debug)]
pub(super) struct WifiEntry {
    pub(super) ssid: String,
    pub(super) signal: u8,
    pub(super) secured: bool,
    pub(super) connected: bool,
}

/// Returns `(wifi_available, networks)`. `wifi_available=false` means we
/// found no wifi interface or no tool that can list networks, which the
/// view renders as a dedicated "no Wi-Fi" placeholder.
pub(super) fn scan_wifi_networks() -> (bool, Vec<WifiEntry>) {
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

pub(super) fn split_nmcli_columns(line: &str) -> Vec<String> {
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

pub(super) fn signal_bars(percent: u8) -> u8 {
    match percent {
        70..=100 => 3,
        40..=69 => 2,
        _ => 1,
    }
}

pub(super) fn wifi_row_view(
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

#[cfg(test)]
mod tests {
    use super::{signal_bars, split_nmcli_columns};

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
