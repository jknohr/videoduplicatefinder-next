//! User-managed "not a match" group blacklist — persistence and filtering.
//!
//! Faithful port of:
//! - VDF.Core/Utils/BlacklistStore.cs    — load/save with atomic write + quarantine
//! - VDF.Core/Utils/GroupBlacklistFilter.cs — compute which duplicate groups are covered
//! - VDF.Core/Utils/AtomicJsonWriter.cs  — write-to-temp + rename pattern
//! - VDF.Core/Utils/PathComparer.cs      — platform-sensitive path comparison

use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};
use tracing::warn;

// ─── Platform-aware path comparison ──────────────────────────────────────────

/// Returns `true` when two path strings should be considered equal on the
/// running OS.  Windows / macOS: case-insensitive.  Linux: case-sensitive.
pub fn paths_equal(a: &str, b: &str) -> bool {
    if cfg!(any(target_os = "windows", target_os = "macos")) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

fn make_path_set(paths: Vec<String>) -> HashSet<String> {
    // Rust's `HashSet` uses the key's `Hash`+`Eq`, so on a case-insensitive OS we
    // fold to lowercase before inserting.  This mirrors C# HashSet(PathComparer).
    if cfg!(any(target_os = "windows", target_os = "macos")) {
        paths.into_iter().map(|p| p.to_lowercase()).collect()
    } else {
        paths.into_iter().collect()
    }
}

fn normalise_path(p: &str) -> String {
    if cfg!(any(target_os = "windows", target_os = "macos")) {
        p.to_lowercase()
    } else {
        p.to_owned()
    }
}

// ─── Blacklist types ──────────────────────────────────────────────────────────

/// One "not a match" entry — a set of paths the user marked as not real duplicates.
pub type BlacklistEntry = HashSet<String>;

/// The full blacklist: a list of path-sets.
pub type Blacklist = Vec<BlacklistEntry>;

// ─── Persistence ─────────────────────────────────────────────────────────────

/// On-disk envelope (v1 format).  Legacy v0 was a raw JSON array of arrays.
#[derive(Debug, Serialize, Deserialize)]
struct Envelope {
    version: u32,
    groups: Vec<Vec<String>>,
}

/// Load the blacklist from `path`.
///
/// Returns an empty list when the file is missing, empty, or unreadable.
/// A corrupt file is renamed aside (`.corrupt-YYYYMMDDHHMMSS`) so the app
/// keeps starting — exactly as in C# `BlacklistStore.Load`.
pub fn load(path: &Path) -> Blacklist {
    if !path.exists() {
        return vec![];
    }
    let text = match std::fs::read_to_string(path) {
        Ok(t) if t.trim().is_empty() => return vec![],
        Ok(t) => t,
        Err(e) => {
            quarantine_corrupt(path, &e.to_string());
            return vec![];
        }
    };

    match parse_json(&text) {
        Ok(bl) => bl,
        Err(e) => {
            quarantine_corrupt(path, &e);
            vec![]
        }
    }
}

/// Save the blacklist to `path` via an atomic write (temp file + rename).
/// Mirrors `AtomicJsonWriter.WriteAsync`.
pub fn save(path: &Path, blacklist: &Blacklist) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let tmp = dir.join(format!(
        "{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("blacklist.json")
    ));

    let envelope = Envelope {
        version: 1,
        groups: blacklist.iter().map(|entry| entry.iter().cloned().collect()).collect(),
    };

    let json = serde_json::to_string_pretty(&envelope)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Remove entries where at least one path no longer exists on disk.
/// Returns the number of entries removed.
pub fn prune_missing(blacklist: &mut Blacklist) -> usize {
    let before = blacklist.len();
    blacklist.retain(|entry| entry.iter().all(|p| Path::new(p).exists()));
    before - blacklist.len()
}

// ─── Group filtering ──────────────────────────────────────────────────────────

/// Returns the set of cluster IDs that are fully covered by some entry in `blacklist`.
///
/// A cluster is "covered" when every current path in the cluster is contained
/// in some single blacklist entry.  Subset semantics: if the user marked
/// {A,B,C} as not-a-match, a later scan finding only {A,B} is also blocked.
///
/// `items`: iterator of (cluster_id, file_path) pairs.
/// `blacklist`: list of path-sets loaded from `load()`.
pub fn compute_blacklisted_ids<'a>(
    items: impl IntoIterator<Item = (u64, &'a str)>,
    blacklist: &Blacklist,
) -> HashSet<u64> {
    if blacklist.is_empty() {
        return HashSet::new();
    }

    // cluster_id → [paths in that cluster]
    let mut cluster_paths: HashMap<u64, Vec<String>> = HashMap::new();
    for (id, path) in items {
        cluster_paths.entry(id).or_default().push(normalise_path(path));
    }

    // Pre-normalise the blacklist sets once.
    let normalised_bl: Vec<HashSet<String>> = blacklist
        .iter()
        .map(|entry| entry.iter().map(|p| normalise_path(p)).collect())
        .collect();

    let mut result = HashSet::new();
    'outer: for (cluster_id, paths) in &cluster_paths {
        for bl_entry in &normalised_bl {
            if paths.iter().all(|p| bl_entry.contains(p)) {
                result.insert(*cluster_id);
                continue 'outer;
            }
        }
    }
    result
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn parse_json(text: &str) -> Result<Blacklist, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| e.to_string())?;

    let raw_groups: Vec<Vec<String>> = match &value {
        // v0: raw array of arrays
        serde_json::Value::Array(_) => {
            serde_json::from_value(value.clone()).map_err(|e| e.to_string())?
        }
        // v1: { "version": N, "groups": [[...], ...] }
        serde_json::Value::Object(obj) => {
            let groups = obj
                .get("groups")
                .ok_or("missing 'groups' key")?;
            serde_json::from_value(groups.clone()).map_err(|e| e.to_string())?
        }
        _ => return Err("unknown blacklist format".to_string()),
    };

    Ok(raw_groups.into_iter().map(make_path_set).collect())
}

fn quarantine_corrupt(path: &Path, reason: &str) {
    use std::time::SystemTime;
    let ts = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let corrupt = path.with_extension(format!("corrupt-{ts}"));
    match std::fs::rename(path, &corrupt) {
        Ok(_) => warn!(
            "blacklist file {:?} was unreadable and moved to {:?}: {}",
            path, corrupt, reason
        ),
        Err(e) => warn!(
            "blacklist file {:?} was unreadable and could not be moved: {} (original: {})",
            path, e, reason
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn compute_blacklisted_fully_covered() {
        let bl: Blacklist = vec![make_path_set(vec![
            "/a/foo.mp4".to_string(),
            "/b/bar.mp4".to_string(),
        ])];
        let items = vec![(1u64, "/a/foo.mp4"), (1u64, "/b/bar.mp4")];
        let blocked = compute_blacklisted_ids(items, &bl);
        assert!(blocked.contains(&1));
    }

    #[test]
    fn compute_blacklisted_partial_subset_blocked() {
        // User marked {A,B,C}; scan finds {A,B} — still covered.
        let bl: Blacklist = vec![make_path_set(vec![
            "/a".to_string(),
            "/b".to_string(),
            "/c".to_string(),
        ])];
        let items = vec![(2u64, "/a"), (2u64, "/b")];
        let blocked = compute_blacklisted_ids(items, &bl);
        assert!(blocked.contains(&2));
    }

    #[test]
    fn compute_blacklisted_not_covered() {
        let bl: Blacklist = vec![make_path_set(vec!["/a".to_string(), "/b".to_string()])];
        let items = vec![(3u64, "/a"), (3u64, "/c")]; // /c not in blacklist
        let blocked = compute_blacklisted_ids(items, &bl);
        assert!(!blocked.contains(&3));
    }

    #[test]
    fn parse_v0_format() {
        let json = r#"[["/a/foo.mp4", "/b/bar.mp4"]]"#;
        let bl = parse_json(json).unwrap();
        assert_eq!(bl.len(), 1);
        assert_eq!(bl[0].len(), 2);
    }

    #[test]
    fn parse_v1_format() {
        let json = r#"{"version":1,"groups":[["/a/foo.mp4","/b/bar.mp4"]]}"#;
        let bl = parse_json(json).unwrap();
        assert_eq!(bl.len(), 1);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blacklist.json");
        let mut bl: Blacklist = vec![make_path_set(vec![
            "/a/foo.mp4".to_string(),
            "/b/bar.mp4".to_string(),
        ])];
        save(&path, &bl).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.len(), 1);
        // Normalised comparison
        let entry: Vec<String> = loaded[0].iter().cloned().collect();
        assert_eq!(loaded[0].len(), 2);
    }
}
