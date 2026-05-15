use crate::theme;
use gpui::{div, Context, IntoElement, ParentElement, Render, Styled, Window};
use std::fs;
use std::time::Duration;
use ztheme::Theme;

pub struct BatteryModule {
    capacity: Option<u8>,
    status: BatteryStatus,
    /// Estimated time to empty (discharging) or to full (charging).
    /// `None` when the battery doesn't expose energy/power counters.
    time_remaining: Option<Duration>,
    device: Option<String>,
}

#[derive(Default, PartialEq, Clone, Copy, Debug)]
pub enum BatteryStatus {
    Charging,
    Discharging,
    Full,
    #[default]
    Unknown,
}

impl BatteryModule {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let device = find_battery();
        cx.observe_global::<Theme>(|_, cx| cx.notify()).detach();
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_secs(30))
                .await;
            let state = this.read_with(cx, |m, _| read_battery(m.device.as_deref()));
            let Ok((cap, status, time)) = state else { break };
            if this
                .update(cx, |m, cx| {
                    if m.capacity != cap || m.status != status || m.time_remaining != time {
                        m.capacity = cap;
                        m.status = status;
                        m.time_remaining = time;
                        cx.notify();
                    }
                })
                .is_err()
            {
                break;
            }
        })
        .detach();

        let (capacity, status, time_remaining) = read_battery(device.as_deref());
        BatteryModule {
            capacity,
            status,
            time_remaining,
            device,
        }
    }

    pub fn capacity(&self) -> Option<u8> {
        self.capacity
    }

    pub fn status(&self) -> BatteryStatus {
        self.status
    }

    pub fn time_remaining(&self) -> Option<Duration> {
        self.time_remaining
    }
}

fn find_battery() -> Option<String> {
    fs::read_dir("/sys/class/power_supply")
        .ok()?
        .flatten()
        .find(|e| e.file_name().to_string_lossy().starts_with("BAT"))
        .map(|e| e.file_name().to_string_lossy().to_string())
}

fn read_battery(device: Option<&str>) -> (Option<u8>, BatteryStatus, Option<Duration>) {
    let Some(bat) = device else {
        return (None, BatteryStatus::Unknown, None);
    };
    let base = format!("/sys/class/power_supply/{bat}");
    let capacity = fs::read_to_string(format!("{base}/capacity"))
        .ok()
        .and_then(|s| s.trim().parse().ok());
    let status = fs::read_to_string(format!("{base}/status"))
        .ok()
        .map(|s| match s.trim() {
            "Charging" => BatteryStatus::Charging,
            "Full" => BatteryStatus::Full,
            "Discharging" | "Not charging" => BatteryStatus::Discharging,
            _ => BatteryStatus::Unknown,
        })
        .unwrap_or_default();
    let time_remaining = read_time_remaining(&base, status);
    (capacity, status, time_remaining)
}

/// Estimate time-to-empty (discharging) or time-to-full (charging) from the
/// battery's energy/charge counters. Sysfs exposes either Wh-based counters
/// (`energy_now`/`energy_full` µWh, `power_now` µW) or Ah-based counters
/// (`charge_now`/`charge_full` µAh, `current_now` µA); dividing units by
/// rate gives hours. Returns `None` for Full/Unknown or a zero rate.
fn read_time_remaining(base: &str, status: BatteryStatus) -> Option<Duration> {
    let read = |f: &str| -> Option<f64> {
        fs::read_to_string(format!("{base}/{f}"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
    };
    // Prefer energy (Wh) counters, fall back to charge (Ah) counters.
    let (now, full, rate) = match (read("energy_now"), read("power_now")) {
        (Some(now), Some(power)) => (now, read("energy_full"), power),
        _ => (read("charge_now")?, read("charge_full"), read("current_now")?),
    };
    if rate <= 0.0 {
        return None;
    }
    let remaining_units = match status {
        BatteryStatus::Discharging => now,
        BatteryStatus::Charging => (full? - now).max(0.0),
        BatteryStatus::Full | BatteryStatus::Unknown => return None,
    };
    let hours = remaining_units / rate;
    if !hours.is_finite() || hours <= 0.0 {
        return None;
    }
    Some(Duration::from_secs_f64(hours * 3600.0))
}

impl Render for BatteryModule {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(cap) = self.capacity else {
            return div();
        };
        let icon = match (&self.status, cap) {
            (BatteryStatus::Charging, _) => "󰂄",
            (_, 0..=10) => "󰁺",
            (_, 11..=30) => "󰁼",
            (_, 31..=60) => "󰁾",
            (_, 61..=90) => "󰂀",
            _ => "󰁹",
        };
        let t = cx.global::<Theme>();
        let color = match cap {
            0..=10 => t.urgent,
            11..=25 => t.warning,
            _ => match &self.status {
                BatteryStatus::Charging => t.success,
                _ => t.fg_dim,
            },
        };
        theme::pill(cx)
            .flex()
            .items_center()
            .gap_1()
            .text_color(color)
            .child(icon.to_string())
            .child(theme::pct_label(cap, color))
    }
}
