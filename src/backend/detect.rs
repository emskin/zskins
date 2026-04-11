use std::sync::Arc;
use crate::backend::{
    WorkspaceBackend,
    sway::SwayBackend,
    ext_workspace::ExtWorkspaceBackend,
};

pub fn detect_backend() -> Option<Arc<dyn WorkspaceBackend>> {
    // 1. SWAYSOCK present?
    if let Ok(path) = std::env::var("SWAYSOCK") {
        if std::path::Path::new(&path).exists() {
            log::info!("detected sway backend (SWAYSOCK={path})");
            return Some(Arc::new(SwayBackend::new()));
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
