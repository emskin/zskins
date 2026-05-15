use crate::theme;
use ddc::Ddc;
use ddc_i2c::I2cDeviceDdc;
use gpui::{div, App, Context, DisplayId, IntoElement, ParentElement, Render, Styled, Window};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI16, Ordering};
use std::sync::Arc;
use std::time::Duration;
use ztheme::Theme;

/// One external monitor controlled over DDC/CI.
#[derive(Clone, Debug)]
pub struct DdciDisplay {
    pub path: PathBuf,
    /// DRM connector name (e.g. `HDMI-A-1`, `DP-1`).
    pub connector: String,
    /// Human label from the EDID "Display Product Name" descriptor.
    /// Falls back to `connector` when the EDID is missing/unparseable.
    pub model: String,
    /// GPUI display this i2c bus belongs to. `None` when we couldn't
    /// match (rare; the connector name → uuid bridge usually works).
    pub display_id: Option<DisplayId>,
}

/// Where brightness lives — internal backlight (laptop) or external monitors
/// over DDC/CI. Detected once at startup.
#[derive(Clone, Debug)]
enum Source {
    Backlight {
        device: String,
        max_brightness: u32,
    },
    Ddci2c {
        displays: Vec<DdciDisplay>,
    },
    None,
}

pub struct BrightnessModule {
    /// Per-display brightness cache, indexed by i2c path. For Backlight
    /// the single entry has an empty `PathBuf` key.
    percents: HashMap<PathBuf, u32>,
    source: Source,
    /// Per-display pending target. `-1` = no pending write.
    ddc_targets: HashMap<PathBuf, Arc<AtomicI16>>,
    /// Per-display wake channel; firing it nudges the worker for that bus.
    ddc_wakes: HashMap<PathBuf, async_channel::Sender<()>>,
    /// Worker thread echo channel: the DDC worker pushes a `(path, pct)`
    /// tuple after every successful setvcp so the GPUI thread can refresh
    /// `percents` without waiting for the next poll cycle.
    worker_echo_tx: async_channel::Sender<(PathBuf, u32)>,
}

impl BrightnessModule {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let source = detect_source(cx);
        let mut percents = HashMap::new();
        initial_read(&source, &mut percents);

        // The DDC worker thread echoes the value it just wrote so the
        // GPUI thread can refresh `percents` without waiting on the 10s
        // poll. Panel-initiated writes set `percents` synchronously in
        // `set_percent`; this channel only carries the *worker*'s
        // confirmation (which may differ from the request if the monitor
        // rounded to a coarse step).
        let (worker_echo_tx, worker_echo_rx) = async_channel::bounded::<(PathBuf, u32)>(32);
        cx.spawn(async move |this, cx| {
            while let Ok((path, pct)) = worker_echo_rx.recv().await {
                if this
                    .update(cx, |m, cx| {
                        if m.percents.get(&path).copied() != Some(pct) {
                            m.percents.insert(path.clone(), pct);
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        // One worker per i2c bus: persistent handle + per-bus serialization
        // (different buses can run concurrently).
        let mut ddc_targets: HashMap<PathBuf, Arc<AtomicI16>> = HashMap::new();
        let mut ddc_wakes: HashMap<PathBuf, async_channel::Sender<()>> = HashMap::new();
        if let Source::Ddci2c { displays } = &source {
            for d in displays {
                let target = Arc::new(AtomicI16::new(-1));
                let (wake_tx, wake_rx) = async_channel::bounded::<()>(1);
                ddc_targets.insert(d.path.clone(), target.clone());
                ddc_wakes.insert(d.path.clone(), wake_tx);
                spawn_ddc_worker(
                    d.path.clone(),
                    target,
                    wake_rx,
                    worker_echo_tx.clone(),
                );
            }
        }

        cx.observe_global::<Theme>(|_, cx| cx.notify()).detach();

        let poll = match source {
            Source::Backlight { .. } => Duration::from_secs(5),
            Source::Ddci2c { .. } => Duration::from_secs(10),
            Source::None => Duration::from_secs(60),
        };
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(poll).await;
            let cached = this.read_with(cx, |m, _| m.source.clone());
            let Ok(src) = cached else { break };
            // Run reads off the GPUI thread; DDC/CI can take ~50ms each.
            let updates = cx
                .background_executor()
                .spawn(async move { poll_read_all(&src) })
                .await;
            if this
                .update(cx, |m, cx| {
                    let mut changed = false;
                    for (key, pct) in updates {
                        if m.percents.get(&key).copied() != Some(pct) {
                            m.percents.insert(key, pct);
                            changed = true;
                        }
                    }
                    if changed {
                        cx.notify();
                    }
                })
                .is_err()
            {
                break;
            }
        })
        .detach();

        BrightnessModule {
            percents,
            source,
            ddc_targets,
            ddc_wakes,
            worker_echo_tx,
        }
    }

    /// "Primary" brightness for the cluster icon — first display's value.
    pub fn percent(&self) -> Option<u32> {
        // Backlight stores under an empty PathBuf; Ddci2c stores under
        // first display's path. Either way, just pick the first value.
        self.percents.values().next().copied()
    }

    /// Read brightness for a specific GPUI `DisplayId` (DDC/CI only).
    pub fn percent_for(&self, display_id: DisplayId) -> Option<u32> {
        let Source::Ddci2c { displays } = &self.source else {
            return None;
        };
        let d = displays
            .iter()
            .find(|d| d.display_id == Some(display_id))?;
        self.percents.get(&d.path).copied()
    }

    /// All known DDC/CI displays. Empty for Backlight or None sources.
    pub fn displays(&self) -> &[DdciDisplay] {
        match &self.source {
            Source::Ddci2c { displays } => displays,
            _ => &[],
        }
    }

    /// Friendly label for a given display: `model (connector)`, or just
    /// `connector` if the EDID didn't carry a product name.
    pub fn label_for(&self, display_id: DisplayId) -> Option<String> {
        let d = self
            .displays()
            .iter()
            .find(|d| d.display_id == Some(display_id))?;
        Some(if d.model == d.connector {
            d.connector.clone()
        } else {
            format!("{} ({})", d.model, d.connector)
        })
    }

    /// Set brightness. `target=None` means "all" (backwards compat for
    /// callers that don't know which display they're targeting).
    ///
    /// For DDC/CI displays the actual setvcp runs on a per-bus worker
    /// thread; the worker echoes the applied value back via
    /// `worker_echo_tx`, which the GPUI thread consumes and writes into
    /// `percents`. Panel UIs that need instant visual feedback maintain
    /// their own per-slider drag cache.
    pub fn set_percent(&self, target: Option<DisplayId>, percent: u32) {
        let pct = percent.min(100);
        match &self.source {
            Source::Backlight { .. } => {
                // brightnessctl writes to /sys; the 5s poll picks it up.
                let _ = self.worker_echo_tx.try_send((PathBuf::new(), pct));
                let _ = Command::new("brightnessctl")
                    .args(["set", &format!("{}%", pct)])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
            }
            Source::Ddci2c { displays } => {
                for d in displays {
                    if let Some(t) = target {
                        if d.display_id != Some(t) {
                            continue;
                        }
                    }
                    if let Some(atomic) = self.ddc_targets.get(&d.path) {
                        atomic.store(pct as i16, Ordering::Release);
                    }
                    if let Some(tx) = self.ddc_wakes.get(&d.path) {
                        let _ = tx.try_send(());
                    }
                }
            }
            Source::None => {}
        }
    }
}

fn initial_read(source: &Source, percents: &mut HashMap<PathBuf, u32>) {
    match source {
        Source::Backlight {
            device,
            max_brightness,
        } => {
            if let Some(p) = read_backlight(device, *max_brightness) {
                percents.insert(PathBuf::new(), p);
            }
        }
        Source::Ddci2c { displays } => {
            for d in displays {
                if let Some(p) = read_ddc(&d.path) {
                    percents.insert(d.path.clone(), p);
                }
            }
        }
        Source::None => {}
    }
}

fn poll_read_all(source: &Source) -> Vec<(PathBuf, u32)> {
    let mut out = Vec::new();
    match source {
        Source::Backlight {
            device,
            max_brightness,
        } => {
            if let Some(p) = read_backlight(device, *max_brightness) {
                out.push((PathBuf::new(), p));
            }
        }
        // DDC/CI poll intentionally skipped — opening a fresh
        // `ddc_i2c::from_i2c_device` here would race the worker thread's
        // persistent handle for the same bus's i2c flock. The worker
        // already echoes back every applied value, and external OSD
        // button changes are rare; we accept the small staleness.
        Source::Ddci2c { .. } => {}
        Source::None => {}
    }
    out
}

fn spawn_ddc_worker(
    path: PathBuf,
    target: Arc<AtomicI16>,
    wake_rx: async_channel::Receiver<()>,
    echo_tx: async_channel::Sender<(PathBuf, u32)>,
) {
    let thread_name = format!(
        "brightness-ddc-{}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("worker")
    );
    std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            // Open the i2c handle once. Reopening per write would double
            // each transaction's latency.
            let mut handle: I2cDeviceDdc = match ddc_i2c::from_i2c_device(&path) {
                Ok(h) => h,
                Err(e) => {
                    tracing::warn!("brightness: open {path:?} failed: {e}");
                    return;
                }
            };
            while wake_rx.recv_blocking().is_ok() {
                while wake_rx.try_recv().is_ok() {}
                let pct = target.swap(-1, Ordering::AcqRel);
                if pct < 0 {
                    continue;
                }
                let value = (pct.max(0) as u16).min(100);
                if let Err(e) = handle.set_vcp_feature(0x10, value) {
                    tracing::debug!("brightness: setvcp on {path:?} failed: {e}");
                }
                let _ = echo_tx.try_send((path.clone(), value as u32));
            }
        })
        .expect("spawn brightness worker");
}

fn detect_source(cx: &App) -> Source {
    if let Some(device) = find_backlight() {
        let max_brightness = fs::read_to_string(format!(
            "/sys/class/backlight/{device}/max_brightness"
        ))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
        if max_brightness > 0 {
            return Source::Backlight {
                device,
                max_brightness,
            };
        }
    }

    let drm_map = read_drm_i2c_map();
    let connector_to_display = crate::backend::name_to_display_id(
        cx,
        drm_map.values().map(|(c, _)| c.as_str()),
    );

    // Probe each i2c device that has a DRM connector. Skip ones that
    // don't respond to a brightness read — that weeds out hubs and KVM
    // pass-throughs that expose i2c but ignore DDC.
    let mut displays = Vec::new();
    for (path, (connector, model)) in drm_map {
        if !path.exists() {
            continue;
        }
        let responsive = ddc_i2c::from_i2c_device(&path)
            .ok()
            .and_then(|mut h| h.get_vcp_feature(0x10).ok())
            .is_some();
        if !responsive {
            continue;
        }
        let display_id = connector_to_display.get(&connector).copied();
        displays.push(DdciDisplay {
            path,
            connector: connector.clone(),
            model: if model.is_empty() {
                connector
            } else {
                model
            },
            display_id,
        });
    }

    if !displays.is_empty() {
        // Stable ordering across runs.
        displays.sort_by(|a, b| a.connector.cmp(&b.connector));
        return Source::Ddci2c { displays };
    }
    Source::None
}

/// Walk `/sys/class/drm/card*-*` and return a map from `/dev/i2c-N` path
/// to `(connector_name, edid_product_name)`. Empty when no displays /
/// no DRM info available.
fn read_drm_i2c_map() -> HashMap<PathBuf, (String, String)> {
    let mut out = HashMap::new();
    let entries = match fs::read_dir("/sys/class/drm") {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        // We want entries shaped like "card1-HDMI-A-1", not "card1" or
        // "renderD128".
        let Some(connector) = strip_card_prefix(&name_str) else {
            continue;
        };
        let dir = entry.path();
        let ddc_link = dir.join("ddc");
        let target = match fs::canonicalize(&ddc_link) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let bus_name = match target.file_name().and_then(|n| n.to_str()) {
            Some(n) if n.starts_with("i2c-") => n.to_string(),
            _ => continue,
        };
        let dev_path = PathBuf::from(format!("/dev/{bus_name}"));
        let model = read_edid_product_name(&dir.join("edid")).unwrap_or_default();
        out.insert(dev_path, (connector.to_string(), model));
    }
    out
}

/// Strip the `cardN-` prefix from a sysfs DRM entry name, returning the
/// connector portion (e.g. `card1-HDMI-A-1` → `Some("HDMI-A-1")`).
fn strip_card_prefix(s: &str) -> Option<&str> {
    if !s.starts_with("card") {
        return None;
    }
    let rest = s.strip_prefix("card")?;
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        if c == '-' {
            return Some(chars.as_str());
        }
        if !c.is_ascii_digit() {
            return None;
        }
    }
    None
}

/// Pull the "Display Product Name" descriptor out of a binary EDID blob.
/// Returns `None` when the file is missing, too short, or no `0xFC`
/// descriptor is present.
fn read_edid_product_name(path: &std::path::Path) -> Option<String> {
    parse_edid_product_name(&fs::read(path).ok()?)
}

/// Parse a binary EDID blob and return the "Display Product Name"
/// (descriptor type `0xFC`). Pure function — no I/O.
fn parse_edid_product_name(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 128 {
        return None;
    }
    // The four 18-byte descriptor blocks start at offset 54. Descriptor
    // type tag is byte 3 of each block; `0xFC` is the product name.
    for block in [54usize, 72, 90, 108] {
        if block + 18 > bytes.len() {
            break;
        }
        let d = &bytes[block..block + 18];
        // Detailed timing descriptors have non-zero pixel clock in bytes 0..2.
        if d[0] != 0 || d[1] != 0 || d[3] != 0xFC {
            continue;
        }
        // Bytes 5..18 are ASCII, padded with newline (0x0A) + spaces.
        let raw: Vec<u8> = d[5..18].iter().take_while(|&&b| b != 0x0A).copied().collect();
        let name = String::from_utf8_lossy(&raw).trim().to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }
    None
}

fn find_backlight() -> Option<String> {
    fs::read_dir("/sys/class/backlight")
        .ok()?
        .flatten()
        .next()
        .map(|e| e.file_name().to_string_lossy().to_string())
}

fn read_backlight(device: &str, max: u32) -> Option<u32> {
    if max == 0 {
        return None;
    }
    let cur: u32 = fs::read_to_string(format!("/sys/class/backlight/{device}/brightness"))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some((cur * 100) / max)
}

fn read_ddc(path: &std::path::Path) -> Option<u32> {
    let mut h = ddc_i2c::from_i2c_device(path).ok()?;
    let v = h.get_vcp_feature(0x10).ok()?;
    let max = v.maximum() as u32;
    if max == 0 {
        return None;
    }
    Some((v.value() as u32 * 100) / max)
}

impl Render for BrightnessModule {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(pct) = self.percent() else {
            return div();
        };
        let icon = match pct {
            0..=30 => "󰃞",
            31..=70 => "󰃟",
            _ => "󰃠",
        };
        let t = cx.global::<Theme>();
        theme::pill(cx)
            .bg(gpui::Hsla::transparent_black())
            .flex()
            .items_center()
            .gap_0p5()
            .child(div().text_color(t.accent).child(icon.to_string()))
            .child(theme::pct_label(pct, t.fg_dim))
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_edid_product_name, strip_card_prefix};

    #[test]
    fn strip_card_prefix_extracts_connector() {
        assert_eq!(strip_card_prefix("card0-HDMI-A-1"), Some("HDMI-A-1"));
        assert_eq!(strip_card_prefix("card1-DP-2"), Some("DP-2"));
        assert_eq!(strip_card_prefix("card12-eDP-1"), Some("eDP-1"));
    }

    #[test]
    fn strip_card_prefix_rejects_invalid() {
        assert_eq!(strip_card_prefix("HDMI-A-1"), None);
        assert_eq!(strip_card_prefix("cardX-A-1"), None); // non-digit after `card`
        assert_eq!(strip_card_prefix("card0HDMI"), None); // no `-` separator
        assert_eq!(strip_card_prefix("card0"), None);
    }

    /// Build a 128-byte EDID with a 0xFC descriptor in block `idx` (0..4)
    /// containing `name` (truncated to 13 bytes, padded with 0x0A + 0x20).
    fn edid_with_name(idx: usize, name: &str) -> Vec<u8> {
        let mut edid = vec![0u8; 128];
        let block = 54 + idx * 18;
        // d[0..3] = 0 (descriptor marker), d[3] = 0xFC (type), d[4] = 0.
        edid[block + 3] = 0xFC;
        let bytes = name.as_bytes();
        let n = bytes.len().min(13);
        edid[block + 5..block + 5 + n].copy_from_slice(&bytes[..n]);
        if n < 13 {
            edid[block + 5 + n] = 0x0A;
            for b in &mut edid[block + 5 + n + 1..block + 18] {
                *b = 0x20;
            }
        }
        edid
    }

    #[test]
    fn parses_product_name_from_first_descriptor() {
        let edid = edid_with_name(0, "JZM27DC");
        assert_eq!(parse_edid_product_name(&edid).as_deref(), Some("JZM27DC"));
    }

    #[test]
    fn parses_product_name_from_later_descriptor() {
        let edid = edid_with_name(2, "Dell U2720Q");
        assert_eq!(parse_edid_product_name(&edid).as_deref(), Some("Dell U2720Q"));
    }

    #[test]
    fn rejects_short_edid() {
        assert_eq!(parse_edid_product_name(&[0u8; 64]), None);
    }

    #[test]
    fn rejects_edid_without_product_descriptor() {
        let edid = vec![0u8; 128];
        assert_eq!(parse_edid_product_name(&edid), None);
    }

    #[test]
    fn skips_detailed_timing_descriptors() {
        // d[0..2] != 0 marks a detailed timing block, not a name descriptor.
        let mut edid = vec![0u8; 128];
        edid[54] = 0x12; // pixel clock low byte non-zero
        edid[54 + 3] = 0xFC; // would-be name type, but should be ignored
        // Real name in the second block.
        let block2 = 72;
        edid[block2 + 3] = 0xFC;
        edid[block2 + 5..block2 + 5 + 6].copy_from_slice(b"REAL27");
        edid[block2 + 5 + 6] = 0x0A;
        assert_eq!(parse_edid_product_name(&edid).as_deref(), Some("REAL27"));
    }

    #[test]
    fn trims_trailing_padding() {
        let edid = edid_with_name(0, "BENQ  "); // trailing spaces before pad
        assert_eq!(parse_edid_product_name(&edid).as_deref(), Some("BENQ"));
    }
}
