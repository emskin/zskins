use crate::theme;
use gpui::{div, Context, IntoElement, ParentElement, Render, Styled, Window};
use std::fs;
use std::process::Command;
use std::time::Duration;

pub struct NetworkModule {
    state: NetState,
}

#[derive(PartialEq)]
enum NetState {
    Wifi { ssid: String },
    Ethernet,
    Disconnected,
}

impl NetworkModule {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_secs(10))
                .await;
            let state = read_network();
            if this
                .update(cx, |m, cx| {
                    if m.state != state {
                        m.state = state;
                        cx.notify();
                    }
                })
                .is_err()
            {
                break;
            }
        })
        .detach();

        NetworkModule {
            state: read_network(),
        }
    }
}

fn read_network() -> NetState {
    if let Some(ssid) = read_wifi_ssid() {
        return NetState::Wifi { ssid };
    }
    if is_ethernet_up() {
        return NetState::Ethernet;
    }
    NetState::Disconnected
}

fn read_wifi_ssid() -> Option<String> {
    let output = Command::new("iwgetid").arg("-r").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let ssid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if ssid.is_empty() {
        None
    } else {
        Some(ssid)
    }
}

fn is_ethernet_up() -> bool {
    let Ok(entries) = fs::read_dir("/sys/class/net") else {
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "lo" || name.starts_with("wl") {
            continue;
        }
        if let Ok(state) = fs::read_to_string(format!("/sys/class/net/{name}/operstate")) {
            if state.trim() == "up" {
                return true;
            }
        }
    }
    false
}

impl Render for NetworkModule {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let (icon, label, icon_color, text_color) = match &self.state {
            NetState::Wifi { ssid } => ("󰤨", ssid.as_str(), theme::green(), theme::fg_dim()),
            NetState::Ethernet => ("󰈀", "Eth", theme::green(), theme::fg_dim()),
            NetState::Disconnected => ("󰤭", "Off", theme::urgent(), theme::urgent()),
        };
        theme::pill()
            .bg(gpui::Hsla::transparent_black())
            .flex()
            .items_center()
            .gap_0p5()
            .child(div().text_color(icon_color).child(icon.to_string()))
            .child(div().text_color(text_color).child(label.to_string()))
    }
}
