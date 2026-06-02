//! Snapshot testing (§spec Phase 9). Record a response on first run, diff it on
//! later runs. Snapshots are plain text files committed to git — regression
//! detection that fits the git-first thesis.

use serde_json::Value;
use similar::TextDiff;
use std::fs;
use std::io;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotResult {
    /// No snapshot existed; one was written.
    Created,
    /// Snapshot existed and matched.
    Match,
    /// Snapshot existed, differed, and was overwritten (update mode).
    Updated,
    /// Snapshot existed and differed; carries a unified diff.
    Mismatch(String),
}

/// Canonical text form of a response body for stable diffs. JSON is pretty-
/// printed with sorted keys (serde_json's default `Map` is ordered), so key
/// reordering doesn't create spurious diffs. Non-JSON is kept verbatim.
pub fn normalize(body: &[u8]) -> String {
    let mut text = match serde_json::from_slice::<Value>(body) {
        Ok(value) => serde_json::to_string_pretty(&value)
            .unwrap_or_else(|_| String::from_utf8_lossy(body).into_owned()),
        Err(_) => String::from_utf8_lossy(body).into_owned(),
    };
    // Trailing newline keeps snapshot files POSIX-clean and git diffs tidy.
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// Compare `current` against the snapshot at `path`. Writes the snapshot when
/// absent (Created) or when `update` is set (Updated); otherwise reports
/// Match/Mismatch.
pub fn check(path: &Path, current: &str, update: bool) -> io::Result<SnapshotResult> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if !path.exists() {
        fs::write(path, current)?;
        return Ok(SnapshotResult::Created);
    }
    let stored = fs::read_to_string(path)?;
    if stored == current {
        return Ok(SnapshotResult::Match);
    }
    if update {
        fs::write(path, current)?;
        return Ok(SnapshotResult::Updated);
    }
    let diff = TextDiff::from_lines(stored.as_str(), current);
    let rendered = diff
        .unified_diff()
        .context_radius(3)
        .header("stored", "current")
        .to_string();
    Ok(SnapshotResult::Mismatch(rendered))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_normalized_canonically() {
        let a = normalize(br#"{"b":2,"a":1}"#);
        let b = normalize(br#"{"a":1,"b":2}"#);
        assert_eq!(a, b, "key order should not matter");
        assert!(a.contains('\n'), "pretty-printed");
    }

    #[test]
    fn create_then_match_then_mismatch() {
        let dir = std::env::temp_dir().join(format!("pg-snap-{}", std::process::id()));
        let path = dir.join("__snapshots__").join("x.snap");
        let _ = fs::remove_dir_all(&dir);

        // first run: created
        assert_eq!(
            check(&path, "hello\nworld", false).unwrap(),
            SnapshotResult::Created
        );
        // same content: match
        assert_eq!(
            check(&path, "hello\nworld", false).unwrap(),
            SnapshotResult::Match
        );
        // changed content: mismatch with a diff
        match check(&path, "hello\nthere", false).unwrap() {
            SnapshotResult::Mismatch(diff) => {
                assert!(diff.contains("world"));
                assert!(diff.contains("there"));
            }
            other => panic!("expected mismatch, got {other:?}"),
        }
        // update mode overwrites
        assert_eq!(
            check(&path, "hello\nthere", true).unwrap(),
            SnapshotResult::Updated
        );
        // and now matches
        assert_eq!(
            check(&path, "hello\nthere", false).unwrap(),
            SnapshotResult::Match
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
