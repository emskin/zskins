//! System integration helpers — leaf wrappers around `Command::spawn`.
//!
//! All commands run non-blocking and silently swallow errors: we don't want
//! UI to block on slow tools, and the next state read on the next panel
//! open will reflect reality anyway.

use std::process::{Command, Stdio};

pub(super) fn nmcli_wifi_enabled() -> Option<bool> {
    let out = Command::new("nmcli")
        .args(["-t", "radio", "wifi"])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    Some(s.trim() == "enabled")
}

pub(super) fn nmcli_set_wifi(on: bool) {
    let _ = Command::new("nmcli")
        .args(["radio", "wifi", if on { "on" } else { "off" }])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

pub(super) fn rfkill_unblocked(kind: &str) -> Option<bool> {
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

pub(super) fn rfkill_set(kind: &str, enabled: bool) {
    let _ = Command::new("rfkill")
        .args([if enabled { "unblock" } else { "block" }, kind])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

pub(super) fn rfkill_all_blocked() -> Option<bool> {
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

pub(super) fn rfkill_set_all(airplane: bool) {
    let _ = Command::new("rfkill")
        .args([if airplane { "block" } else { "unblock" }, "all"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// Best-effort night-light detection — checks whether gammastep/wlsunset/
/// redshift is running. We don't try to introspect their state.
pub(super) fn gammastep_running() -> bool {
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
pub(super) fn toggle_night_light(enable: bool) {
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

pub(super) fn spawn_settings_app() {
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

pub(super) fn spawn_lock_screen() {
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

pub(super) fn spawn_power_menu() {
    // No standardized power-menu app; for now just trigger logout via
    // loginctl. A proper implementation would open a confirmation popup
    // with logout/restart/shutdown options.
    let _ = Command::new("loginctl")
        .args(["terminate-user", &std::env::var("USER").unwrap_or_default()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}
