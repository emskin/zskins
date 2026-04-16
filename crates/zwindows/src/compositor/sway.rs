//! Sway (and i3) backend — delegates to the existing `sway_tree`
//! walker so we don't duplicate the i3-ipc wire format.

use super::{CompositorIpc, FocusedWindow};

pub struct SwayIpc;

impl CompositorIpc for SwayIpc {
    fn focused_window(&self) -> Option<FocusedWindow> {
        crate::sway_tree::focused_window_with_workspace()
            .ok()
            .flatten()
            .map(|(app_id, title, workspace)| FocusedWindow {
                app_id,
                title,
                workspace,
            })
    }
}
