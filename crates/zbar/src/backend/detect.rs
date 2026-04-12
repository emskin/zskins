use crate::backend::{ext_workspace::ExtWorkspaceBackend, sway::SwayBackend, WorkspaceBackend};
use std::sync::Arc;

pub fn detect_backend() -> Option<Arc<dyn WorkspaceBackend>> {
    // 1. SWAYSOCK present?
    if let Ok(path) = std::env::var("SWAYSOCK") {
        if std::path::Path::new(&path).exists() {
            log::info!("detected sway backend (SWAYSOCK={path})");
            return Some(Arc::new(SwayBackend));
        }
    }

    // 2. ext-workspace-v1 advertised?
    if ExtWorkspaceBackend::probe() {
        log::info!("detected ext-workspace-v1 backend");
        return Some(Arc::new(ExtWorkspaceBackend::new()));
    }

    log::warn!("no workspace backend available");
    None
}
