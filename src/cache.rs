//! On-disk cache of ffprobe results, keyed by (path, size, mtime).

use crate::probe::Track;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    size: u64,
    mtime_ms: u64,
    tracks: Vec<Track>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Stored {
    version: u32,
    entries: HashMap<String, Entry>,
}

/// Probe cache. A disabled cache never hits and never writes.
#[derive(Debug, Default)]
pub struct Cache {
    path: Option<PathBuf>,
    entries: HashMap<String, Entry>,
    dirty: bool,
}

impl Cache {
    /// A cache that does nothing (`--no-cache`).
    pub fn disabled() -> Self {
        Self::default()
    }

    /// Load from `path`. A missing, unreadable or outdated file just means an empty cache.
    pub fn load(path: &Path) -> Self {
        let entries = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Stored>(&s).ok())
            .filter(|s| s.version == VERSION)
            .map(|s| s.entries)
            .unwrap_or_default();
        Self {
            path: Some(path.to_path_buf()),
            entries,
            dirty: false,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.path.is_some()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Cached tracks, only if size and mtime still match.
    pub fn get(&self, key: &str, size: u64, mtime_ms: u64) -> Option<&[Track]> {
        if !self.is_enabled() {
            return None;
        }
        self.entries
            .get(key)
            .filter(|e| e.size == size && e.mtime_ms == mtime_ms)
            .map(|e| e.tracks.as_slice())
    }

    pub fn insert(&mut self, key: String, size: u64, mtime_ms: u64, tracks: Vec<Track>) {
        if !self.is_enabled() {
            return;
        }
        self.entries.insert(
            key,
            Entry {
                size,
                mtime_ms,
                tracks,
            },
        );
        self.dirty = true;
    }

    /// Write the cache if anything changed. Writes to a temp file first, then renames.
    pub fn save(&mut self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if !self.dirty {
            return Ok(());
        }
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let stored = Stored {
            version: VERSION,
            entries: std::mem::take(&mut self.entries),
        };
        let json = serde_json::to_string(&stored).map_err(std::io::Error::other)?;
        self.entries = stored.entries;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        self.dirty = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::Lang;

    fn track(lang: Lang) -> Track {
        Track {
            lang,
            language_tag: None,
            codec: None,
            title: None,
            default: false,
            commentary: false,
        }
    }

    #[test]
    fn hit_requires_matching_size_and_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = Cache::load(&dir.path().join("c.json"));
        c.insert("a.mkv".into(), 100, 5, vec![track(Lang::English)]);
        assert!(c.get("a.mkv", 100, 5).is_some());
        assert!(c.get("a.mkv", 101, 5).is_none());
        assert!(c.get("a.mkv", 100, 6).is_none());
        assert!(c.get("b.mkv", 100, 5).is_none());
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("c.json");
        let mut c = Cache::load(&path);
        c.insert("a.mkv".into(), 1, 2, vec![track(Lang::Japanese)]);
        c.save().unwrap();
        let again = Cache::load(&path);
        assert_eq!(again.len(), 1);
        assert_eq!(again.get("a.mkv", 1, 2).unwrap()[0].lang, Lang::Japanese);
    }

    #[test]
    fn missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let c = Cache::load(&dir.path().join("nope.json"));
        assert!(c.is_empty());
        assert!(c.is_enabled());
    }

    #[test]
    fn corrupt_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        std::fs::write(&path, "{ this is not json").unwrap();
        assert!(Cache::load(&path).is_empty());
    }

    #[test]
    fn wrong_version_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        std::fs::write(&path, r#"{"version":999,"entries":{}}"#).unwrap();
        assert!(Cache::load(&path).is_empty());
    }

    #[test]
    fn disabled_cache_never_hits_or_writes() {
        let mut c = Cache::disabled();
        c.insert("a".into(), 1, 1, vec![]);
        assert!(c.get("a", 1, 1).is_none());
        assert!(c.is_empty());
        c.save().unwrap();
    }

    #[test]
    fn save_without_changes_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        let mut c = Cache::load(&path);
        c.save().unwrap();
        assert!(!path.exists());
    }
}
