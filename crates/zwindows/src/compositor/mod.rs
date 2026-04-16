//! Compositor IPC abstraction.
//!
//! Each supported Wayland compositor exposes its "currently focused window"
//! via a different private IPC socket. Callers don't want that in their
//! face — they just want `(app_id, title)` of whichever window was focused
//! before zofi grabbed input.
//!
//! The trait deliberately stays narrow: one method today, with room for
//! workspace/pid extensions later (see `zskins#14`). Each backend silently
//! degrades to `None` on any error — a missing compositor IPC is the
//! common case, not an exceptional one, and crash-on-failure would make
//! zofi unusable outside the one compositor we happened to test on.

mod hyprland;
mod noop;
mod sway;

pub use hyprland::HyprlandIpc;
pub use noop::NoopIpc;
pub use sway::SwayIpc;

/// Snapshot of the focused toplevel returned by a compositor IPC backend.
/// `workspace` is optional because not every backend exposes it (and not
/// every focused-thing has one — sway can focus a workspace background).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusedWindow {
    pub app_id: String,
    pub title: String,
    pub workspace: Option<String>,
}

/// One-way read interface to whatever compositor is running. Implementers
/// live in the sibling modules; pick one at runtime via [`detect`].
pub trait CompositorIpc: Send + Sync {
    /// Window holding keyboard focus when called, or `None` if nothing
    /// is focused or the IPC failed.
    fn focused_window(&self) -> Option<FocusedWindow>;
}

/// Pick the first compositor backend whose detection signal is set in the
/// environment. Detection happens in a fixed order: sway → Hyprland →
/// noop. The returned trait object is always usable — the noop fallback
/// just answers `None` so callers don't need to special-case "no
/// compositor detected".
pub fn detect() -> Box<dyn CompositorIpc> {
    if std::env::var("SWAYSOCK").is_ok() || std::env::var("I3SOCK").is_ok() {
        tracing::info!("compositor::detect chose sway");
        return Box::new(SwayIpc);
    }
    if std::env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok() {
        tracing::info!("compositor::detect chose hyprland");
        return Box::new(HyprlandIpc);
    }
    tracing::info!("compositor::detect chose noop (no known compositor env)");
    Box::new(NoopIpc)
}
