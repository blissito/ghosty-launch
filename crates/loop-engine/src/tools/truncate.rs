//! SHA-addressed tool-output spillover, extracted from
//! `ghostycode/crates/tui/src/tools/truncate.rs`.
//!
//! The vendored chat wire-builder dedups repeated large tool results to a
//! `<TOOL_RESULT_REF sha="..." />` and persists the original bytes here so the
//! model can retrieve them. Only the SHA path is needed by the engine, so the
//! tool-call-id spillover, boot-prune, and `ToolResult` integration from the
//! original are dropped. `write_atomic` is replaced with `std::fs::write`
//! (the dedup path only needs the bytes on disk, not crash-atomicity).

use std::fs;
use std::io;
use std::path::PathBuf;

/// Name of the spillover directory under the home dir.
pub const SPILLOVER_DIR_NAME: &str = "tool_outputs";

#[cfg(test)]
static TEST_SPILLOVER_ROOT: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

#[cfg(test)]
pub(crate) static TEST_SPILLOVER_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Resolve `~/.ghosty/tool_outputs/` (falling back to the legacy
/// `~/.deepseek/tool_outputs/`). Returns `None` if the home directory can't be
/// determined; callers treat that as "spillover unavailable" and degrade.
#[must_use]
pub fn spillover_root() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(root) = TEST_SPILLOVER_ROOT
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .clone()
    {
        return Some(root);
    }

    let home = dirs::home_dir()?;
    let primary = home.join(".ghosty").join(SPILLOVER_DIR_NAME);
    let legacy = home.join(".deepseek").join(SPILLOVER_DIR_NAME);
    if primary.exists() || !legacy.exists() {
        return Some(primary);
    }
    Some(legacy)
}

/// Override the spillover root for tests without mutating `$HOME`.
#[cfg(test)]
pub(crate) fn set_test_spillover_root(root: Option<PathBuf>) -> Option<PathBuf> {
    let mut guard = TEST_SPILLOVER_ROOT
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    std::mem::replace(&mut *guard, root)
}

/// Resolve the spillover-file path for a SHA256 content hash. `sha` must be the
/// raw 64-char lowercase hex digest.
#[must_use]
pub fn sha_spillover_path(sha: &str) -> Option<PathBuf> {
    let sha = sha.trim().to_ascii_lowercase();
    if !is_valid_sha256(&sha) {
        return None;
    }
    Some(spillover_root()?.join(format!("sha_{sha}.txt")))
}

/// True when `s` is a 64-character lowercase ASCII hex string.
#[must_use]
pub fn is_valid_sha256(s: &str) -> bool {
    s.len() == 64
        && s.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// Write content to the SHA-addressed spillover file. Idempotent — the same
/// hash maps to the same path, and skips the write when the file already
/// exists (the common case for the wire dedup's second sighting).
pub fn write_sha_spillover(sha: &str, content: &str) -> io::Result<PathBuf> {
    let path = sha_spillover_path(sha).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "sha must be a 64-char lowercase hex digest",
        )
    })?;
    if path.exists() {
        return Ok(path);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, content.as_bytes())?;
    Ok(path)
}
