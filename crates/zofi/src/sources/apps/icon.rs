use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use rayon::prelude::*;

use super::desktop::DesktopEntry;

static NEXT_IMAGE_ID: AtomicU64 = AtomicU64::new(1);

/// Resolve icon paths and preload all icon bytes into memory.
pub fn resolve_icons(mut entries: Vec<DesktopEntry>) -> Vec<DesktopEntry> {
    let cache = icon_theme::IconCache::new(&["apps"]);

    // Step 1: assign paths.
    for entry in &mut entries {
        if let Some(ref name) = entry.icon_name {
            if name.starts_with('/') {
                let p = PathBuf::from(name);
                if p.exists() {
                    entry.icon_path = Some(p);
                }
            } else {
                entry.icon_path = cache.lookup(name).map(Path::to_path_buf);
            }
        }
    }

    // Step 2: parallel-read all icon files into memory.
    let loaded: Vec<Option<Arc<gpui::Image>>> = entries
        .par_iter()
        .map(|entry| {
            let path = entry.icon_path.as_ref()?;
            let bytes = fs::read(path).ok()?;
            let format = format_from_ext(path)?;
            Some(Arc::new(gpui::Image {
                format,
                bytes,
                id: NEXT_IMAGE_ID.fetch_add(1, Ordering::Relaxed),
            }))
        })
        .collect();

    for (entry, data) in entries.iter_mut().zip(loaded) {
        if data.is_some() {
            entry.icon_path = None;
        }
        entry.icon_data = data;
    }

    let preloaded = entries.iter().filter(|e| e.icon_data.is_some()).count();
    tracing::info!("{preloaded}/{} icons preloaded into memory", entries.len());

    entries
}

fn format_from_ext(path: &Path) -> Option<gpui::ImageFormat> {
    match path.extension()?.to_str()? {
        "svg" | "svgz" => Some(gpui::ImageFormat::Svg),
        "png" => Some(gpui::ImageFormat::Png),
        "jpg" | "jpeg" => Some(gpui::ImageFormat::Jpeg),
        _ => None,
    }
}
