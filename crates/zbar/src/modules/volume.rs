use crate::theme;
use gpui::{div, Context, IntoElement, ParentElement, Render, Styled, Window};
use std::process::Command;
use std::time::Duration;

pub struct VolumeModule {
    percent: Option<u32>,
    muted: bool,
}

impl VolumeModule {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (percent, muted) = read_volume();

        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let (percent, muted) = read_volume();
            if this
                .update(cx, |m, cx| {
                    if m.percent != percent || m.muted != muted {
                        m.percent = percent;
                        m.muted = muted;
                        cx.notify();
                    }
                })
                .is_err()
            {
                break;
            }
        })
        .detach();

        VolumeModule { percent, muted }
    }
}

fn read_volume() -> (Option<u32>, bool) {
    if let Some(result) = read_wpctl() {
        return result;
    }
    read_pactl().unwrap_or((None, false))
}

fn read_wpctl() -> Option<(Option<u32>, bool)> {
    let output = Command::new("wpctl")
        .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let muted = text.contains("[MUTED]");
    let vol = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<f32>().ok())
        .map(|v| (v * 100.0).round() as u32);
    Some((vol, muted))
}

fn read_pactl() -> Option<(Option<u32>, bool)> {
    let output = Command::new("pactl")
        .args(["get-sink-volume", "@DEFAULT_SINK@"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let vol = text
        .split('/')
        .nth(1)
        .and_then(|s| s.trim().trim_end_matches('%').parse().ok());

    let mute_output = Command::new("pactl")
        .args(["get-sink-mute", "@DEFAULT_SINK@"])
        .output()
        .ok()?;
    let muted = String::from_utf8_lossy(&mute_output.stdout).contains("yes");

    Some((vol, muted))
}

impl Render for VolumeModule {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let Some(vol) = self.percent else {
            return div();
        };
        let (icon, icon_color) = if self.muted {
            ("󰝟", theme::urgent())
        } else {
            match vol {
                0 => ("󰕿", theme::fg_dim()),
                1..=50 => ("󰖀", theme::accent()),
                _ => ("󰕾", theme::accent()),
            }
        };
        let text_color = if self.muted {
            theme::urgent()
        } else {
            theme::fg_dim()
        };
        theme::pill()
            .bg(gpui::Hsla::transparent_black())
            .flex()
            .items_center()
            .gap_0p5()
            .child(div().text_color(icon_color).child(icon.to_string()))
            .child(theme::pct_label(vol, text_color))
    }
}
