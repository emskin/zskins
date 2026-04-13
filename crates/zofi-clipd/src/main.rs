use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use zofi_clipd_core::{daemon, db::Db, model::Kind, paths, pidfile::DaemonLock, preview};

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        None => run_daemon(),
        Some("import") => {
            let path = args
                .next()
                .map(PathBuf::from)
                .context("usage: zofi-clipd import <path>")?;
            import(&path)
        }
        Some(other) => {
            eprintln!("zofi-clipd: unknown subcommand `{other}`");
            eprintln!("usage:");
            eprintln!("  zofi-clipd                run the daemon");
            eprintln!("  zofi-clipd import <path>  import clipboard history (clipman.json)");
            std::process::exit(2);
        }
    }
}

fn run_daemon() -> Result<()> {
    let pid_path = paths::pid_path();
    let _lock = DaemonLock::acquire(&pid_path).context("acquire daemon lock")?;

    let db_path = paths::db_path()?;
    let db = Db::open(&db_path)?;
    tracing::info!("db={db_path:?} pid={pid_path:?}");

    daemon::run(db)
}

/// Import a clipman-style JSON array of strings into the db. Each string
/// becomes a text entry. Older items get older `created_at` so they sort
/// after fresh entries.
fn import(path: &PathBuf) -> Result<()> {
    let bytes = std::fs::read(path).with_context(|| format!("read {path:?}"))?;
    let items: Vec<String> =
        serde_json::from_slice(&bytes).context("parse JSON array of strings")?;
    if items.is_empty() {
        eprintln!("no entries in {path:?}");
        return Ok(());
    }

    let db = Db::open(&paths::db_path()?)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    // clipman appears to store newest first. Insert oldest first so the most
    // recent ends up with the highest last_used_at.
    let mut inserted = 0;
    let mut deduped = 0;
    let mut converted = 0;
    for (rev_ix, raw) in items.iter().rev().enumerate() {
        if raw.is_empty() {
            continue;
        }
        let bytes = raw.as_bytes();
        let normalized = match decode_utf16_if_likely(bytes) {
            Some(s) => {
                converted += 1;
                s.into_bytes()
            }
            None => bytes.to_vec(),
        };
        if normalized.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(&normalized);
        let preview = preview::build(&text);
        let ts = now + rev_ix as i64;
        match db.record_with_ts(
            Kind::Text,
            "text/plain;charset=utf-8",
            &normalized,
            Some(&preview),
            &[],
            ts,
        ) {
            Ok(zofi_clipd_core::db::RecordResult::Inserted(_)) => inserted += 1,
            Ok(zofi_clipd_core::db::RecordResult::Existed(_)) => deduped += 1,
            Err(e) => tracing::warn!("skip entry {}: {e:#}", rev_ix),
        }
    }
    eprintln!(
        "imported {inserted} entries ({deduped} duplicates collapsed, \
         {converted} converted from UTF-16) from {path:?}"
    );
    Ok(())
}

/// Heuristic: clipman occasionally stores UTF-16LE blobs from apps (Firefox,
/// some Java apps). If `bytes` looks like UTF-16LE — even length, BOM or many
/// odd-position NULs — decode it; otherwise leave it alone.
fn decode_utf16_if_likely(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 4 || !bytes.len().is_multiple_of(2) {
        return None;
    }
    let stripped = if bytes.starts_with(&[0xFF, 0xFE]) {
        &bytes[2..]
    } else {
        // Heuristic: if at least 70% of odd-indexed bytes in the first 32
        // pairs are zero, treat it as UTF-16LE.
        let sample = bytes.len().min(64);
        let pairs = sample / 2;
        if pairs == 0 {
            return None;
        }
        let nuls = (0..pairs).filter(|i| bytes[i * 2 + 1] == 0).count();
        if nuls * 10 < pairs * 7 {
            return None;
        }
        bytes
    };
    let units: Vec<u16> = stripped
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let s = String::from_utf16(&units).ok()?;
    Some(s.trim_end_matches('\0').to_string())
}

