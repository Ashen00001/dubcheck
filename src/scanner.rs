//! Walks the library (read-only) and probes files in parallel.

use crate::cache::Cache;
use crate::parser::{parse_episode, parse_season_folder, Parsed};
use crate::probe::{Prober, Track};
use anyhow::{Context, Result};
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

const VIDEO_EXTS: [&str; 10] = [
    "mkv", "mp4", "m4v", "avi", "webm", "mov", "ts", "wmv", "mpg", "mpeg",
];

const SKIP_DIRS: [&str; 9] = [
    "extras",
    "featurettes",
    "samples",
    "sample",
    "trailers",
    "behind the scenes",
    "extrafanart",
    "metadata",
    "subs",
];

pub const ROOT_SHOW: &str = "(library root)";

/// What we know about a file before probing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Episode { season: u32, episodes: Vec<u32> },
    Unparsed,
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub abs: PathBuf,
    /// Path relative to the library root, always with `/` separators.
    pub rel: String,
    pub show: String,
    pub size: u64,
    pub mtime_ms: u64,
    pub kind: Kind,
}

#[derive(Debug, Default)]
pub struct Discovery {
    pub files: Vec<FileEntry>,
    /// Extras, samples, openings and so on that were skipped on purpose.
    pub ignored: usize,
    pub warnings: Vec<String>,
}

fn is_skipped_dir(name: &str) -> bool {
    name.starts_with('.') || SKIP_DIRS.contains(&name.to_ascii_lowercase().as_str())
}

fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

/// List the video files under `root`. Only reads directory entries and file metadata.
pub fn discover(root: &Path, show_filter: Option<&str>) -> Result<Discovery> {
    anyhow::ensure!(root.is_dir(), "'{}' is not a directory", root.display());
    let filter = show_filter.map(str::to_lowercase);
    let mut out = Discovery::default();

    let walker = WalkDir::new(root)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| {
            e.depth() == 0
                || !e.file_type().is_dir()
                || !is_skipped_dir(&e.file_name().to_string_lossy())
        });

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                out.warnings.push(e.to_string());
                continue;
            }
        };
        if !entry.file_type().is_file() || !is_video(entry.path()) {
            continue;
        }
        let rel_path = entry.path().strip_prefix(root).unwrap_or(entry.path());
        let comps: Vec<String> = rel_path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let show = if comps.len() > 1 {
            comps[0].clone()
        } else {
            ROOT_SHOW.to_owned()
        };
        if let Some(f) = &filter {
            if !show.to_lowercase().contains(f) {
                continue;
            }
        }
        let folder_season = comps
            .iter()
            .take(comps.len().saturating_sub(1))
            .skip(1)
            .rev()
            .find_map(|c| parse_season_folder(c));
        let stem = entry
            .path()
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();

        let kind = match parse_episode(&stem) {
            Parsed::Extra => {
                out.ignored += 1;
                continue;
            }
            Parsed::Unknown => Kind::Unparsed,
            Parsed::Episode { season, episodes } => Kind::Episode {
                season: season.or(folder_season).unwrap_or(1),
                episodes,
            },
        };

        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(e) => {
                out.warnings.push(e.to_string());
                continue;
            }
        };
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));

        out.files.push(FileEntry {
            abs: entry.path().to_path_buf(),
            rel: comps.join("/"),
            show,
            size: meta.len(),
            mtime_ms,
            kind,
        });
    }
    Ok(out)
}

/// A file plus the outcome of probing it.
#[derive(Debug)]
pub struct Probed {
    pub entry: FileEntry,
    /// `Err` holds a message for files ffprobe could not read.
    pub tracks: Result<Vec<Track>, String>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ProbeStats {
    pub cached: usize,
    pub probed: usize,
}

/// Probe every parsed file, using the cache where size and mtime still match.
pub fn probe_all<P: Prober>(
    entries: Vec<FileEntry>,
    prober: &P,
    cache: &mut Cache,
    jobs: usize,
) -> Result<(Vec<Probed>, ProbeStats)> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs.max(1))
        .build()
        .context("could not start worker threads")?;

    let shared: &Cache = cache;
    let results: Vec<(Probed, bool)> = pool.install(|| {
        entries
            .into_par_iter()
            .map(|entry| {
                if entry.kind == Kind::Unparsed {
                    return (
                        Probed {
                            entry,
                            tracks: Ok(Vec::new()),
                        },
                        true,
                    );
                }
                let key = entry.abs.to_string_lossy().into_owned();
                if let Some(t) = shared.get(&key, entry.size, entry.mtime_ms) {
                    let tracks = Ok(t.to_vec());
                    return (Probed { entry, tracks }, true);
                }
                let tracks = prober.probe(&entry.abs).map_err(|e| e.to_string());
                (Probed { entry, tracks }, false)
            })
            .collect()
    });

    let mut stats = ProbeStats::default();
    let mut probed = Vec::with_capacity(results.len());
    for (p, from_cache) in results {
        if p.entry.kind != Kind::Unparsed {
            if from_cache {
                stats.cached += 1;
            } else {
                stats.probed += 1;
                if let Ok(t) = &p.tracks {
                    cache.insert(
                        p.entry.abs.to_string_lossy().into_owned(),
                        p.entry.size,
                        p.entry.mtime_ms,
                        t.clone(),
                    );
                }
            }
        }
        probed.push(p);
    }
    Ok((probed, stats))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::{Lang, ProbeError};
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn touch(root: &Path, rel: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"x").unwrap();
    }

    fn find<'a>(d: &'a Discovery, rel: &str) -> &'a FileEntry {
        d.files.iter().find(|f| f.rel == rel).unwrap()
    }

    #[test]
    fn discovers_layouts() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        touch(r, "Moon Garden/Season 01/Moon Garden - S01E01.mkv");
        touch(r, "Moon Garden/Season 2/Moon Garden - 03.mkv");
        touch(r, "Moon Garden/Specials/Moon Garden - OVA 1.mkv");
        touch(
            r,
            "Starlight Academy/[Grp] Starlight Academy - 05 [1080p].mkv",
        );
        touch(r, "Starlight Academy/readme.txt");
        touch(r, "Starlight Academy/Season 01/weird name.mkv");
        let d = discover(r, None).unwrap();
        assert_eq!(d.files.len(), 5);

        let f = find(&d, "Moon Garden/Season 01/Moon Garden - S01E01.mkv");
        assert_eq!(f.show, "Moon Garden");
        assert_eq!(
            f.kind,
            Kind::Episode {
                season: 1,
                episodes: vec![1]
            }
        );
        // season comes from the folder when the name has none
        assert_eq!(
            find(&d, "Moon Garden/Season 2/Moon Garden - 03.mkv").kind,
            Kind::Episode {
                season: 2,
                episodes: vec![3]
            }
        );
        // OVA keyword wins over folder
        assert_eq!(
            find(&d, "Moon Garden/Specials/Moon Garden - OVA 1.mkv").kind,
            Kind::Episode {
                season: 0,
                episodes: vec![1]
            }
        );
        // no season folder defaults to season 1
        assert_eq!(
            find(
                &d,
                "Starlight Academy/[Grp] Starlight Academy - 05 [1080p].mkv"
            )
            .kind,
            Kind::Episode {
                season: 1,
                episodes: vec![5]
            }
        );
        assert_eq!(
            find(&d, "Starlight Academy/Season 01/weird name.mkv").kind,
            Kind::Unparsed
        );
    }

    #[test]
    fn filename_season_beats_folder() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "Show/Season 01/Show - S02E04.mkv");
        let d = discover(dir.path(), None).unwrap();
        assert_eq!(
            d.files[0].kind,
            Kind::Episode {
                season: 2,
                episodes: vec![4]
            }
        );
    }

    #[test]
    fn extras_and_skip_dirs_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        touch(r, "Show/Season 01/Show - S01E01.mkv");
        touch(r, "Show/Season 01/Show - NCOP1.mkv");
        touch(r, "Show/Extras/Making Of.mkv");
        touch(r, "Show/Season 01/sample/Show - S01E01 sample.mkv");
        let d = discover(r, None).unwrap();
        assert_eq!(d.files.len(), 1);
        assert_eq!(d.ignored, 1);
    }

    #[test]
    fn show_filter_is_case_insensitive_substring() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        touch(r, "Moon Garden/Moon Garden - S01E01.mkv");
        touch(r, "Starlight Academy/Starlight Academy - S01E01.mkv");
        let d = discover(r, Some("MOON")).unwrap();
        assert_eq!(d.files.len(), 1);
        assert_eq!(d.files[0].show, "Moon Garden");
    }

    #[test]
    fn files_in_root_get_placeholder_show() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "Loose - S01E01.mkv");
        let d = discover(dir.path(), None).unwrap();
        assert_eq!(d.files[0].show, ROOT_SHOW);
    }

    #[test]
    fn not_a_directory_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.mkv");
        fs::write(&f, b"x").unwrap();
        assert!(discover(&f, None).is_err());
        assert!(discover(&dir.path().join("missing"), None).is_err());
    }

    struct Fake {
        calls: AtomicUsize,
    }
    impl Prober for Fake {
        fn probe(&self, path: &Path) -> Result<Vec<Track>, ProbeError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if path.to_string_lossy().contains("broken") {
                return Err(ProbeError::Failed {
                    code: "1".into(),
                    stderr: "bad file".into(),
                });
            }
            Ok(vec![Track {
                lang: Lang::English,
                language_tag: Some("eng".into()),
                codec: None,
                title: None,
                default: true,
                commentary: false,
            }])
        }
    }

    #[test]
    fn second_run_uses_cache_and_failures_are_reported() {
        let lib = tempfile::tempdir().unwrap();
        let cache_dir = tempfile::tempdir().unwrap();
        touch(lib.path(), "Show/Show - S01E01.mkv");
        touch(lib.path(), "Show/Show - S01E02 broken.mkv");
        touch(lib.path(), "Show/mystery.mkv");

        let fake = Fake {
            calls: AtomicUsize::new(0),
        };
        let cache_path = cache_dir.path().join("c.json");

        let mut cache = Cache::load(&cache_path);
        let d = discover(lib.path(), None).unwrap();
        let (probed, stats) = probe_all(d.files, &fake, &mut cache, 2).unwrap();
        cache.save().unwrap();
        assert_eq!(fake.calls.load(Ordering::SeqCst), 2);
        assert_eq!(stats.probed, 2);
        assert_eq!(probed.iter().filter(|p| p.tracks.is_err()).count(), 1);

        // second run: the good file is cached, the broken one is retried, unparsed not probed
        let mut cache = Cache::load(&cache_path);
        let d = discover(lib.path(), None).unwrap();
        let (_, stats) = probe_all(d.files, &fake, &mut cache, 2).unwrap();
        assert_eq!(stats.cached, 1);
        assert_eq!(stats.probed, 1);
        assert_eq!(fake.calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn changed_file_is_reprobed() {
        let lib = tempfile::tempdir().unwrap();
        let cache_dir = tempfile::tempdir().unwrap();
        touch(lib.path(), "Show/Show - S01E01.mkv");
        let fake = Fake {
            calls: AtomicUsize::new(0),
        };
        let cache_path = cache_dir.path().join("c.json");

        let mut cache = Cache::load(&cache_path);
        let d = discover(lib.path(), None).unwrap();
        probe_all(d.files, &fake, &mut cache, 1).unwrap();
        cache.save().unwrap();

        fs::write(lib.path().join("Show/Show - S01E01.mkv"), b"longer content").unwrap();
        let mut cache = Cache::load(&cache_path);
        let d = discover(lib.path(), None).unwrap();
        let (_, stats) = probe_all(d.files, &fake, &mut cache, 1).unwrap();
        assert_eq!(stats.probed, 1);
    }

    #[test]
    fn no_cache_always_probes() {
        let lib = tempfile::tempdir().unwrap();
        touch(lib.path(), "Show/Show - S01E01.mkv");
        let fake = Fake {
            calls: AtomicUsize::new(0),
        };
        for _ in 0..2 {
            let mut cache = Cache::disabled();
            let d = discover(lib.path(), None).unwrap();
            probe_all(d.files, &fake, &mut cache, 1).unwrap();
        }
        assert_eq!(fake.calls.load(Ordering::SeqCst), 2);
    }
}
