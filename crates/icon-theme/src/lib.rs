//! Freedesktop icon theme lookup with category-based caching.
//!
//! ```no_run
//! let cache = icon_theme::IconCache::new(&["apps"]);
//! if let Some(path) = cache.lookup("firefox") {
//!     println!("found: {}", path.display());
//! }
//! ```

use rayon::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

const EXTENSIONS: &[&str] = &["svg", "svgz", "png"];

/// Size subdirectories in priority order (best first).
const SIZE_DIRS: &[&str] = &[
    "scalable", "48x48", "32x32", "64x64", "24x24", "96x96", "128x128", "256x256", "22x22",
    "16x16", "512x512",
];

/// Breeze-style size subdirectories.
const BREEZE_SIZES: &[&str] = &["48", "32", "64", "22", "16"];

/// A cached name→path mapping for freedesktop icon theme icons.
pub struct IconCache {
    map: HashMap<String, PathBuf>,
}

impl IconCache {
    /// Build an icon cache by scanning system icon themes for the given categories.
    ///
    /// Categories are freedesktop icon subdirectories like `"apps"`, `"status"`,
    /// `"devices"`, `"actions"`, etc. Earlier categories have higher priority.
    pub fn new(categories: &[&str]) -> Self {
        let themes = detect_themes();
        tracing::info!("icon themes: {themes:?}");

        let dirs = collect_scan_dirs(&themes, categories);
        let map = build_cache(dirs);
        tracing::info!("icon cache: {} icons indexed", map.len());

        Self { map }
    }

    /// Look up an icon by its stem name (e.g. `"firefox"`, `"network-wireless"`).
    pub fn lookup(&self, name: &str) -> Option<&Path> {
        self.map.get(name).map(|p| p.as_path())
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Theme detection
// ---------------------------------------------------------------------------

/// Determine theme search order: user theme → Adwaita → hicolor.
fn detect_themes() -> Vec<String> {
    let mut themes = Vec::new();

    if let Some(t) = read_gtk_icon_theme() {
        add_theme_with_parents(&t, &mut themes);
    }

    for fallback in &["Adwaita", "gnome", "hicolor"] {
        add_theme_with_parents(fallback, &mut themes);
    }

    themes
}

/// Add a theme and its Inherits= parents, deduplicating.
fn add_theme_with_parents(name: &str, themes: &mut Vec<String>) {
    if themes.iter().any(|t| t == name) {
        return;
    }
    let root = PathBuf::from("/usr/share/icons").join(name);
    if !root.is_dir() {
        return;
    }
    themes.push(name.to_string());

    let index = root.join("index.theme");
    if let Ok(content) = fs::read_to_string(&index) {
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("Inherits=") {
                for parent in rest.split(',') {
                    let parent = parent.trim();
                    if !parent.is_empty() {
                        add_theme_with_parents(parent, themes);
                    }
                }
                break;
            }
        }
    }
}

fn read_gtk_icon_theme() -> Option<String> {
    let config = dirs_config();
    for path in &[
        config.join("gtk-3.0/settings.ini"),
        config.join("gtk-4.0/settings.ini"),
    ] {
        if let Ok(content) = fs::read_to_string(path) {
            for line in content.lines() {
                if let Some(rest) = line.strip_prefix("gtk-icon-theme-name") {
                    let val = rest.trim_start_matches(['=', ' '].as_ref()).trim();
                    if !val.is_empty() {
                        return Some(val.to_string());
                    }
                }
            }
        }
    }
    None
}

fn dirs_config() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config")
        })
}

// ---------------------------------------------------------------------------
// Directory scanning
// ---------------------------------------------------------------------------

/// Collect all icon directories to scan, ordered by (priority, path).
fn collect_scan_dirs(themes: &[String], categories: &[&str]) -> Vec<(usize, PathBuf)> {
    let mut dirs = Vec::new();

    for (theme_idx, theme) in themes.iter().enumerate() {
        let root = PathBuf::from("/usr/share/icons").join(theme);

        for (cat_idx, cat) in categories.iter().enumerate() {
            // Standard layout: {root}/{size}/{category}/
            for (size_idx, size) in SIZE_DIRS.iter().enumerate() {
                let dir = root.join(size).join(cat);
                if dir.is_dir() {
                    dirs.push((theme_idx * 10000 + cat_idx * 100 + size_idx, dir));
                }
            }

            // Breeze layout: {root}/{category}/{size}/
            let cat_dir = root.join(cat);
            if cat_dir.is_dir() {
                for (size_idx, sz) in BREEZE_SIZES.iter().enumerate() {
                    let dir = cat_dir.join(sz);
                    if dir.is_dir() {
                        dirs.push((theme_idx * 10000 + cat_idx * 100 + 50 + size_idx, dir));
                    }
                }
            }
        }
    }

    // /usr/share/pixmaps — lowest priority fallback.
    let pixmaps = PathBuf::from("/usr/share/pixmaps");
    if pixmaps.is_dir() {
        dirs.push((usize::MAX, pixmaps));
    }

    dirs
}

/// Parallel-scan directories and build a name→path map (lowest priority wins).
fn build_cache(scan_dirs: Vec<(usize, PathBuf)>) -> HashMap<String, PathBuf> {
    let found: Vec<(String, PathBuf, usize)> = scan_dirs
        .par_iter()
        .flat_map(|(priority, dir)| {
            let mut results = Vec::new();
            if let Ok(rd) = fs::read_dir(dir) {
                for entry in rd.flatten() {
                    let path = entry.path();
                    if !is_icon_file(&path) {
                        continue;
                    }
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        results.push((stem.to_string(), path, *priority));
                    }
                }
            }
            results
        })
        .collect();

    let mut cache: HashMap<String, (PathBuf, usize)> = HashMap::with_capacity(found.len());
    for (name, path, priority) in found {
        cache
            .entry(name)
            .and_modify(|existing| {
                if priority < existing.1 {
                    *existing = (path.clone(), priority);
                }
            })
            .or_insert((path, priority));
    }

    cache.into_iter().map(|(k, (v, _))| (k, v)).collect()
}

fn is_icon_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| EXTENSIONS.iter().any(|&e| e.eq_ignore_ascii_case(ext)))
}
