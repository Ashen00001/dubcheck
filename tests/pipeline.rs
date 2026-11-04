//! End-to-end test with a fake prober and a throwaway library of empty files.

use clap::Parser;
use dubcheck::cache::Cache;
use dubcheck::cli::{scan, Cli};
use dubcheck::probe::{parse_probe_json, ProbeError, Prober, Track};
use dubcheck::report::{render, EXIT_NO_ENGLISH, EXIT_STRUCTURAL};
use std::fs;
use std::path::Path;

const DUAL: &str = r#"{"streams":[
  {"codec_type":"video","codec_name":"hevc"},
  {"codec_type":"audio","codec_name":"aac","tags":{"language":"jpn"}},
  {"codec_type":"audio","codec_name":"aac","tags":{"language":"eng"}}]}"#;
const SUB: &str = r#"{"streams":[
  {"codec_type":"audio","codec_name":"flac","tags":{"language":"jpn"}}]}"#;

/// Picks a fixture from the file name: "sub" in the name means sub-only, otherwise dual.
struct FixtureProber;

impl Prober for FixtureProber {
    fn probe(&self, path: &Path) -> Result<Vec<Track>, ProbeError> {
        let name = path.file_name().unwrap().to_string_lossy().to_lowercase();
        parse_probe_json(if name.contains("sub") { SUB } else { DUAL })
    }
}

fn touch(root: &Path, rel: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, b"not really a video").unwrap();
}

fn snapshot(root: &Path) -> Vec<(String, u64)> {
    let mut v: Vec<_> = walkdir::WalkDir::new(root)
        .into_iter()
        .map(|e| e.unwrap())
        .map(|e| (e.path().display().to_string(), e.metadata().unwrap().len()))
        .collect();
    v.sort();
    v
}

#[test]
fn full_pipeline_over_a_fake_library() {
    let lib = tempfile::tempdir().unwrap();
    let r = lib.path();
    touch(r, "Moon Garden/Season 01/Moon Garden - S01E01.mkv");
    touch(r, "Moon Garden/Season 01/Moon Garden - S01E02.mkv");
    touch(r, "Moon Garden/Season 01/Moon Garden - S01E04 sub.mkv");
    touch(r, "Moon Garden/Season 01/Moon Garden - S01E04 copy.mkv");
    touch(
        r,
        "Starlight Academy/[Grp] Starlight Academy - 01 [1080p] sub.mkv",
    );
    touch(
        r,
        "Starlight Academy/[Grp] Starlight Academy - 02 [1080p] sub.mkv",
    );
    touch(r, "Starlight Academy/notes about this show.mkv");
    touch(r, "Starlight Academy/Extras/Behind The Scenes.mkv");

    let before = snapshot(r);

    let cli = Cli::parse_from(["dubcheck", "ignored"]);
    let (rep, _) = scan(r, &cli, &FixtureProber, &mut Cache::disabled()).unwrap();

    assert_eq!(rep.summary.shows, 2);
    assert_eq!(rep.summary.episodes, 5);
    assert_eq!(rep.summary.missing_episodes, 1);
    assert_eq!(rep.summary.duplicate_episodes, 1);
    assert_eq!(rep.summary.unparsed_files, 1);
    assert_eq!(rep.summary.counts.dual, 3);
    assert_eq!(rep.summary.counts.japanese, 2);
    assert_eq!(rep.exit_code(), EXIT_STRUCTURAL | EXIT_NO_ENGLISH);
    assert_eq!(rep.no_english[0].show, "Starlight Academy");

    let text = render(&rep, false);
    assert!(text.contains("Moon Garden"));
    assert!(text.contains("missing 3"));
    assert!(text.contains("notes about this show.mkv"));

    // the library must be untouched
    assert_eq!(snapshot(r), before);
}

#[test]
fn only_missing_dub_flag_trims_output() {
    let lib = tempfile::tempdir().unwrap();
    touch(lib.path(), "Good Show/Good Show - S01E01.mkv");
    touch(lib.path(), "Bad Show/Bad Show - S01E01 sub.mkv");
    let cli = Cli::parse_from(["dubcheck", "x", "--only-missing-dub"]);
    let (rep, _) = scan(lib.path(), &cli, &FixtureProber, &mut Cache::disabled()).unwrap();
    assert_eq!(rep.shows.len(), 1);
    assert_eq!(rep.shows[0].name, "Bad Show");
}

#[test]
fn show_flag_filters_by_substring() {
    let lib = tempfile::tempdir().unwrap();
    touch(lib.path(), "Good Show/Good Show - S01E01.mkv");
    touch(lib.path(), "Bad Show/Bad Show - S01E01.mkv");
    let cli = Cli::parse_from(["dubcheck", "x", "--show", "good"]);
    let (rep, _) = scan(lib.path(), &cli, &FixtureProber, &mut Cache::disabled()).unwrap();
    assert_eq!(rep.summary.shows, 1);
}

#[test]
fn flags_parse() {
    let cli = Cli::parse_from([
        "dubcheck",
        "lib",
        "--json",
        "--no-cache",
        "--ffprobe",
        "/x/ffprobe",
        "--jobs",
        "2",
    ]);
    assert!(cli.json && cli.no_cache);
    assert_eq!(cli.jobs, 2);
    assert_eq!(cli.ffprobe.unwrap().to_str().unwrap(), "/x/ffprobe");
}
