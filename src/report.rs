//! Turns probed files into a per-show/per-season report and renders it.

use crate::probe::AudioStatus;
use crate::scanner::{Kind, Probed};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Exit-code bits (combined with OR): 0 is clean, 1 is reserved for runtime errors.
pub const EXIT_STRUCTURAL: u8 = 2;
pub const EXIT_NO_ENGLISH: u8 = 4;

const MAP_CAP: usize = 100;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub dual: usize,
    pub english: usize,
    pub japanese: usize,
    pub unknown: usize,
}

impl Counts {
    fn add(&mut self, s: AudioStatus) {
        match s {
            AudioStatus::Dual => self.dual += 1,
            AudioStatus::English => self.english += 1,
            AudioStatus::Japanese => self.japanese += 1,
            AudioStatus::Unknown => self.unknown += 1,
        }
    }

    pub fn total(&self) -> usize {
        self.dual + self.english + self.japanese + self.unknown
    }

    fn merge(&mut self, o: &Counts) {
        self.dual += o.dual;
        self.english += o.english;
        self.japanese += o.japanese;
        self.unknown += o.unknown;
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EpisodeCopy {
    pub file: String,
    pub status: AudioStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct EpisodeReport {
    pub number: u32,
    /// Best status among the copies.
    pub status: AudioStatus,
    pub copies: Vec<EpisodeCopy>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeasonReport {
    pub season: u32,
    pub episodes: Vec<EpisodeReport>,
    pub missing: Vec<u32>,
    pub duplicates: Vec<u32>,
    pub counts: Counts,
    /// One character per episode slot: D dual, E english, J japanese, ? unknown, . missing.
    pub map: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShowReport {
    pub name: String,
    pub seasons: Vec<SeasonReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbeFailure {
    pub file: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NoEnglishGroup {
    pub show: String,
    pub season: u32,
    pub episodes: Vec<u32>,
    pub season_total: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Summary {
    pub shows: usize,
    pub seasons: usize,
    pub episodes: usize,
    pub files: usize,
    pub counts: Counts,
    pub missing_episodes: usize,
    pub duplicate_episodes: usize,
    pub unparsed_files: usize,
    pub probe_errors: usize,
    pub no_english_episodes: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub shows: Vec<ShowReport>,
    pub unparsed: Vec<String>,
    pub probe_errors: Vec<ProbeFailure>,
    /// Seasons lacking English audio, worst first.
    pub no_english: Vec<NoEnglishGroup>,
    /// Always covers everything scanned, even when `--only-missing-dub` trims `shows`.
    pub summary: Summary,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BuildOptions {
    /// Only look for gaps between the lowest and highest episode present, instead of from 1.
    pub lenient_start: bool,
}

type EpisodeMap = BTreeMap<u32, Vec<EpisodeCopy>>;

fn status_char(s: AudioStatus) -> char {
    match s {
        AudioStatus::Dual => 'D',
        AudioStatus::English => 'E',
        AudioStatus::Japanese => 'J',
        AudioStatus::Unknown => '?',
    }
}

fn build_season(season: u32, eps: EpisodeMap, opts: BuildOptions) -> SeasonReport {
    let mut episodes = Vec::new();
    let mut duplicates = Vec::new();
    let mut counts = Counts::default();
    for (number, mut copies) in eps {
        copies.sort_by(|a, b| a.file.cmp(&b.file));
        copies.dedup_by(|a, b| a.file == b.file);
        let status = copies
            .iter()
            .map(|c| c.status)
            .max_by_key(|s| s.rank())
            .unwrap_or(AudioStatus::Unknown);
        if copies.len() > 1 {
            duplicates.push(number);
        }
        counts.add(status);
        episodes.push(EpisodeReport {
            number,
            status,
            copies,
        });
    }

    let mut missing = Vec::new();
    // Specials have no fixed sequence, so gaps only make sense for real seasons.
    if season > 0 {
        if let (Some(first), Some(last)) = (episodes.first(), episodes.last()) {
            let start = if opts.lenient_start { first.number } else { 1 };
            let present: std::collections::BTreeSet<u32> =
                episodes.iter().map(|e| e.number).collect();
            missing = (start..=last.number)
                .filter(|n| !present.contains(n))
                .collect();
        }
    }

    let mut map = String::new();
    let mut slots = 0usize;
    let mut push_slot = |c: char| {
        if slots < MAP_CAP {
            map.push(c);
        }
        slots += 1;
    };
    if season > 0 && !episodes.is_empty() {
        let start = if opts.lenient_start {
            episodes[0].number
        } else {
            1
        };
        let last = episodes.last().map_or(0, |e| e.number);
        let by_num: BTreeMap<u32, AudioStatus> =
            episodes.iter().map(|e| (e.number, e.status)).collect();
        for n in start..=last {
            push_slot(by_num.get(&n).map_or('.', |s| status_char(*s)));
        }
    } else {
        for e in &episodes {
            push_slot(status_char(e.status));
        }
    }
    if slots > MAP_CAP {
        let _ = write!(map, "+{}", slots - MAP_CAP);
    }

    SeasonReport {
        season,
        episodes,
        missing,
        duplicates,
        counts,
        map,
    }
}

/// Build the report from probed files.
pub fn build(probed: &[Probed], opts: BuildOptions) -> Report {
    let mut tree: BTreeMap<(String, String), BTreeMap<u32, EpisodeMap>> = BTreeMap::new();
    let mut unparsed = Vec::new();
    let mut probe_errors = Vec::new();
    let mut files = 0usize;

    for p in probed {
        match &p.entry.kind {
            Kind::Unparsed => unparsed.push(p.entry.rel.clone()),
            Kind::Episode { season, episodes } => {
                files += 1;
                let status = match &p.tracks {
                    Ok(t) => AudioStatus::from_tracks(t),
                    Err(e) => {
                        probe_errors.push(ProbeFailure {
                            file: p.entry.rel.clone(),
                            error: e.clone(),
                        });
                        AudioStatus::Unknown
                    }
                };
                let show = tree
                    .entry((p.entry.show.to_lowercase(), p.entry.show.clone()))
                    .or_default();
                let season_map = show.entry(*season).or_default();
                for ep in episodes {
                    season_map.entry(*ep).or_default().push(EpisodeCopy {
                        file: p.entry.rel.clone(),
                        status,
                    });
                }
            }
        }
    }
    unparsed.sort();
    probe_errors.sort_by(|a, b| a.file.cmp(&b.file));

    let mut shows = Vec::new();
    for ((_, name), seasons) in tree {
        let seasons = seasons
            .into_iter()
            .map(|(s, eps)| build_season(s, eps, opts))
            .collect();
        shows.push(ShowReport { name, seasons });
    }

    let mut summary = Summary {
        shows: shows.len(),
        files,
        unparsed_files: unparsed.len(),
        probe_errors: probe_errors.len(),
        ..Summary::default()
    };
    let mut no_english = Vec::new();
    for show in &shows {
        summary.seasons += show.seasons.len();
        for s in &show.seasons {
            summary.counts.merge(&s.counts);
            summary.missing_episodes += s.missing.len();
            summary.duplicate_episodes += s.duplicates.len();
            let lacking: Vec<u32> = s
                .episodes
                .iter()
                .filter(|e| !e.status.has_english())
                .map(|e| e.number)
                .collect();
            if !lacking.is_empty() {
                summary.no_english_episodes += lacking.len();
                no_english.push(NoEnglishGroup {
                    show: show.name.clone(),
                    season: s.season,
                    episodes: lacking,
                    season_total: s.episodes.len(),
                });
            }
        }
    }
    summary.episodes = summary.counts.total();
    no_english.sort_by(|a, b| {
        b.episodes
            .len()
            .cmp(&a.episodes.len())
            .then_with(|| a.show.to_lowercase().cmp(&b.show.to_lowercase()))
            .then_with(|| a.season.cmp(&b.season))
    });

    Report {
        shows,
        unparsed,
        probe_errors,
        no_english,
        summary,
    }
}

impl Report {
    /// Bitmask: [`EXIT_STRUCTURAL`] for gaps, duplicates, unparsed or unreadable files,
    /// [`EXIT_NO_ENGLISH`] when any episode has no English audio. 0 means clean.
    pub fn exit_code(&self) -> u8 {
        let s = &self.summary;
        let mut code = 0;
        if s.missing_episodes + s.duplicate_episodes + s.unparsed_files + s.probe_errors > 0 {
            code |= EXIT_STRUCTURAL;
        }
        if s.no_english_episodes > 0 {
            code |= EXIT_NO_ENGLISH;
        }
        code
    }

    /// Keep only seasons that have at least one episode without English audio.
    pub fn retain_missing_dub(&mut self) {
        for show in &mut self.shows {
            show.seasons
                .retain(|s| s.episodes.iter().any(|e| !e.status.has_english()));
        }
        self.shows.retain(|s| !s.seasons.is_empty());
    }
}

/// Format `[1,2,3,5,7,8]` as `1-3, 5, 7-8`.
pub fn ranges(nums: &[u32]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < nums.len() {
        let start = nums[i];
        let mut end = start;
        while i + 1 < nums.len() && nums[i + 1] == end + 1 {
            i += 1;
            end = nums[i];
        }
        out.push(if start == end {
            start.to_string()
        } else {
            format!("{start}-{end}")
        });
        i += 1;
    }
    out.join(", ")
}

fn paint(text: &str, code: &str, color: bool) -> String {
    if color && !text.is_empty() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

fn paint_map(map: &str, color: bool) -> String {
    if !color {
        return map.to_owned();
    }
    map.chars()
        .map(|c| match c {
            'D' => paint("D", "32", true),
            'E' => paint("E", "36", true),
            'J' => paint("J", "33", true),
            '?' | '.' => paint(&c.to_string(), "31", true),
            other => other.to_string(),
        })
        .collect()
}

fn season_label(n: u32) -> String {
    if n == 0 {
        "Specials".to_owned()
    } else {
        format!("S{n:02}")
    }
}

struct Row {
    show: String,
    season: String,
    eps: usize,
    counts: Counts,
    notes: String,
    map: String,
}

/// Render the report as an aligned text table. `color` adds ANSI colors.
pub fn render(report: &Report, color: bool) -> String {
    let mut rows = Vec::new();
    for show in &report.shows {
        for (i, s) in show.seasons.iter().enumerate() {
            let mut notes = Vec::new();
            if !s.missing.is_empty() {
                notes.push(format!("missing {}", ranges(&s.missing)));
            }
            if !s.duplicates.is_empty() {
                notes.push(format!("dupes {}", ranges(&s.duplicates)));
            }
            rows.push(Row {
                show: if i == 0 {
                    show.name.clone()
                } else {
                    String::new()
                },
                season: season_label(s.season),
                eps: s.episodes.len(),
                counts: s.counts,
                notes: notes.join("; "),
                map: s.map.clone(),
            });
        }
    }

    let w_show = rows
        .iter()
        .map(|r| r.show.chars().count())
        .chain(["Show".len()])
        .max()
        .unwrap_or(4);
    let w_notes = rows
        .iter()
        .map(|r| r.notes.chars().count())
        .chain(["Notes".len()])
        .max()
        .unwrap_or(5);

    let mut out = String::new();
    if report.shows.is_empty() {
        out.push_str("No episodes to show.\n");
    } else {
        let header = format!(
            "{:<w_show$}  {:<8}  {:>4}  {:>4}  {:>4}  {:>4}  {:>3}  {:<w_notes$}  Map",
            "Show", "Season", "Eps", "Dual", "Eng", "Jpn", "?", "Notes"
        );
        let _ = writeln!(out, "{}", paint(&header, "1", color));
        for r in &rows {
            let notes = format!("{:<w_notes$}", r.notes);
            let _ = writeln!(
                out,
                "{:<w_show$}  {:<8}  {:>4}  {:>4}  {:>4}  {:>4}  {:>3}  {}  {}",
                r.show,
                r.season,
                r.eps,
                r.counts.dual,
                r.counts.english,
                r.counts.japanese,
                r.counts.unknown,
                paint(&notes, "31", color),
                paint_map(&r.map, color),
            );
        }
        let _ = writeln!(
            out,
            "\nMap: D dual, E english only, J japanese only, ? unknown, . missing"
        );
    }

    if !report.unparsed.is_empty() {
        let _ = writeln!(
            out,
            "\n{}",
            paint(
                &format!("Unrecognised file names ({})", report.unparsed.len()),
                "1;33",
                color
            )
        );
        for f in &report.unparsed {
            let _ = writeln!(out, "  {f}");
        }
    }
    if !report.probe_errors.is_empty() {
        let _ = writeln!(
            out,
            "\n{}",
            paint(
                &format!(
                    "Files ffprobe could not read ({})",
                    report.probe_errors.len()
                ),
                "1;31",
                color
            )
        );
        for e in &report.probe_errors {
            let _ = writeln!(out, "  {}: {}", e.file, e.error);
        }
    }

    let s = &report.summary;
    let _ = writeln!(out, "\n{}", paint("Summary", "1", color));
    let _ = writeln!(
        out,
        "  {} shows, {} seasons, {} episodes in {} files",
        s.shows, s.seasons, s.episodes, s.files
    );
    let _ = writeln!(
        out,
        "  dual {}  english-only {}  japanese-only {}  unknown {}",
        s.counts.dual, s.counts.english, s.counts.japanese, s.counts.unknown
    );
    let _ = writeln!(
        out,
        "  missing {}  duplicates {}  unrecognised {}  unreadable {}",
        s.missing_episodes, s.duplicate_episodes, s.unparsed_files, s.probe_errors
    );

    if !report.no_english.is_empty() {
        let _ = writeln!(
            out,
            "\n{}",
            paint(
                &format!(
                    "No English audio: {} episodes (worst first)",
                    s.no_english_episodes
                ),
                "1;31",
                color
            )
        );
        for g in &report.no_english {
            let _ = writeln!(
                out,
                "  {:>4} of {:<4} {} {}: {}",
                g.episodes.len(),
                g.season_total,
                g.show,
                season_label(g.season),
                ranges(&g.episodes)
            );
        }
    } else if s.episodes > 0 {
        let _ = writeln!(out, "\nEvery episode has English audio.");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::{Lang, Track};
    use crate::scanner::FileEntry;
    use std::path::PathBuf;

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

    fn file(show: &str, season: u32, eps: &[u32], name: &str, langs: &[Lang]) -> Probed {
        Probed {
            entry: FileEntry {
                abs: PathBuf::from(name),
                rel: format!("{show}/{name}"),
                show: show.to_owned(),
                size: 1,
                mtime_ms: 1,
                kind: Kind::Episode {
                    season,
                    episodes: eps.to_vec(),
                },
            },
            tracks: Ok(langs.iter().map(|l| track(*l)).collect()),
        }
    }

    const DUAL: [Lang; 2] = [Lang::Japanese, Lang::English];
    const SUB: [Lang; 1] = [Lang::Japanese];
    const DUB: [Lang; 1] = [Lang::English];

    fn season<'a>(r: &'a Report, show: &str, n: u32) -> &'a SeasonReport {
        r.shows
            .iter()
            .find(|s| s.name == show)
            .unwrap()
            .seasons
            .iter()
            .find(|s| s.season == n)
            .unwrap()
    }

    #[test]
    fn ranges_formatting() {
        assert_eq!(ranges(&[]), "");
        assert_eq!(ranges(&[4]), "4");
        assert_eq!(ranges(&[1, 2, 3, 5, 7, 8]), "1-3, 5, 7-8");
        assert_eq!(ranges(&[2, 4, 6]), "2, 4, 6");
    }

    #[test]
    fn detects_gaps() {
        let p = vec![
            file("Moon Garden", 1, &[1], "e1.mkv", &DUAL),
            file("Moon Garden", 1, &[2], "e2.mkv", &DUAL),
            file("Moon Garden", 1, &[5], "e5.mkv", &DUAL),
            file("Moon Garden", 1, &[8], "e8.mkv", &DUAL),
        ];
        let r = build(&p, BuildOptions::default());
        let s = season(&r, "Moon Garden", 1);
        assert_eq!(s.missing, vec![3, 4, 6, 7]);
        assert_eq!(s.map, "DD..D..D");
        assert_eq!(r.summary.missing_episodes, 4);
        assert_eq!(r.exit_code() & EXIT_STRUCTURAL, EXIT_STRUCTURAL);
    }

    #[test]
    fn missing_start_counts_unless_lenient() {
        let p = vec![
            file("Moon Garden", 2, &[3], "e3.mkv", &DUAL),
            file("Moon Garden", 2, &[4], "e4.mkv", &DUAL),
        ];
        let strict = build(&p, BuildOptions::default());
        assert_eq!(season(&strict, "Moon Garden", 2).missing, vec![1, 2]);
        let lenient = build(
            &p,
            BuildOptions {
                lenient_start: true,
            },
        );
        assert!(season(&lenient, "Moon Garden", 2).missing.is_empty());
        assert_eq!(season(&lenient, "Moon Garden", 2).map, "DD");
    }

    #[test]
    fn specials_have_no_gaps() {
        let p = vec![
            file("Moon Garden", 0, &[1], "s1.mkv", &SUB),
            file("Moon Garden", 0, &[4], "s4.mkv", &SUB),
        ];
        let r = build(&p, BuildOptions::default());
        let s = season(&r, "Moon Garden", 0);
        assert!(s.missing.is_empty());
        assert_eq!(s.map, "JJ");
    }

    #[test]
    fn detects_duplicates_and_keeps_best_status() {
        let p = vec![
            file("Moon Garden", 1, &[1], "a.mkv", &SUB),
            file("Moon Garden", 1, &[1], "b.mkv", &DUAL),
            file("Moon Garden", 1, &[2], "c.mkv", &SUB),
        ];
        let r = build(&p, BuildOptions::default());
        let s = season(&r, "Moon Garden", 1);
        assert_eq!(s.duplicates, vec![1]);
        assert_eq!(s.episodes[0].copies.len(), 2);
        assert_eq!(s.episodes[0].status, AudioStatus::Dual);
        assert_eq!(s.counts.dual, 1);
        assert_eq!(s.counts.japanese, 1);
        assert_eq!(r.summary.duplicate_episodes, 1);
    }

    #[test]
    fn multi_episode_file_covers_all_its_episodes() {
        let p = vec![
            file("Moon Garden", 1, &[1, 2], "e1-2.mkv", &DUAL),
            file("Moon Garden", 1, &[3], "e3.mkv", &DUAL),
        ];
        let r = build(&p, BuildOptions::default());
        let s = season(&r, "Moon Garden", 1);
        assert!(s.missing.is_empty());
        assert!(s.duplicates.is_empty());
        assert_eq!(s.episodes.len(), 3);
        assert_eq!(r.summary.files, 2);
        assert_eq!(r.summary.episodes, 3);
    }

    #[test]
    fn counts_each_status() {
        let p = vec![
            file("A", 1, &[1], "1.mkv", &DUAL),
            file("A", 1, &[2], "2.mkv", &DUB),
            file("A", 1, &[3], "3.mkv", &SUB),
            file("A", 1, &[4], "4.mkv", &[]),
        ];
        let r = build(&p, BuildOptions::default());
        let c = season(&r, "A", 1).counts;
        assert_eq!((c.dual, c.english, c.japanese, c.unknown), (1, 1, 1, 1));
        assert_eq!(r.summary.no_english_episodes, 2);
        assert_eq!(season(&r, "A", 1).map, "DEJ?");
    }

    #[test]
    fn no_english_sorted_worst_first() {
        let p = vec![
            file("Small", 1, &[1], "1.mkv", &SUB),
            file("Small", 1, &[2], "2.mkv", &DUAL),
            file("Big", 1, &[1], "1.mkv", &SUB),
            file("Big", 1, &[2], "2.mkv", &SUB),
            file("Big", 1, &[3], "3.mkv", &SUB),
            file("Mid", 1, &[1], "1.mkv", &SUB),
            file("Mid", 1, &[2], "2.mkv", &SUB),
        ];
        let r = build(&p, BuildOptions::default());
        let order: Vec<&str> = r.no_english.iter().map(|g| g.show.as_str()).collect();
        assert_eq!(order, vec!["Big", "Mid", "Small"]);
        assert_eq!(r.no_english[0].episodes, vec![1, 2, 3]);
        assert_eq!(r.no_english[2].season_total, 2);
    }

    #[test]
    fn unparsed_and_probe_errors_are_collected() {
        let mut bad = file("A", 1, &[2], "2.mkv", &[]);
        bad.tracks = Err("boom".into());
        let mut weird = file("A", 1, &[], "weird.mkv", &[]);
        weird.entry.kind = Kind::Unparsed;
        let p = vec![file("A", 1, &[1], "1.mkv", &DUAL), bad, weird];
        let r = build(&p, BuildOptions::default());
        assert_eq!(r.unparsed, vec!["A/weird.mkv"]);
        assert_eq!(r.probe_errors.len(), 1);
        assert_eq!(r.probe_errors[0].error, "boom");
        assert_eq!(r.summary.files, 2);
        assert_eq!(r.exit_code() & EXIT_STRUCTURAL, EXIT_STRUCTURAL);
    }

    #[test]
    fn exit_codes() {
        let clean = build(
            &[file("A", 1, &[1], "1.mkv", &DUAL)],
            BuildOptions::default(),
        );
        assert_eq!(clean.exit_code(), 0);

        let sub_only = build(
            &[file("A", 1, &[1], "1.mkv", &SUB)],
            BuildOptions::default(),
        );
        assert_eq!(sub_only.exit_code(), EXIT_NO_ENGLISH);

        let both = build(
            &[
                file("A", 1, &[1], "1.mkv", &SUB),
                file("A", 1, &[3], "3.mkv", &DUAL),
            ],
            BuildOptions::default(),
        );
        assert_eq!(both.exit_code(), EXIT_STRUCTURAL | EXIT_NO_ENGLISH);

        let gap_only = build(
            &[
                file("A", 1, &[1], "1.mkv", &DUAL),
                file("A", 1, &[3], "3.mkv", &DUAL),
            ],
            BuildOptions::default(),
        );
        assert_eq!(gap_only.exit_code(), EXIT_STRUCTURAL);
    }

    #[test]
    fn only_missing_dub_trims_shows_but_not_summary() {
        let p = vec![
            file("Good", 1, &[1], "1.mkv", &DUAL),
            file("Mixed", 1, &[1], "1.mkv", &DUAL),
            file("Mixed", 2, &[1], "1.mkv", &SUB),
        ];
        let mut r = build(&p, BuildOptions::default());
        r.retain_missing_dub();
        assert_eq!(r.shows.len(), 1);
        assert_eq!(r.shows[0].name, "Mixed");
        assert_eq!(r.shows[0].seasons.len(), 1);
        assert_eq!(r.shows[0].seasons[0].season, 2);
        assert_eq!(r.summary.shows, 2);
        assert_eq!(r.summary.episodes, 3);
    }

    #[test]
    fn long_map_is_capped() {
        let p: Vec<Probed> = (1..=130)
            .map(|n| file("Long", 1, &[n], &format!("{n}.mkv"), &DUAL))
            .collect();
        let r = build(&p, BuildOptions::default());
        let m = &season(&r, "Long", 1).map;
        assert_eq!(m.len(), MAP_CAP + 3);
        assert!(m.ends_with("+30"));
    }

    fn sample() -> Report {
        let p = vec![
            file("Moon Garden", 1, &[1], "1.mkv", &DUAL),
            file("Moon Garden", 1, &[2], "2.mkv", &DUAL),
            file("Moon Garden", 1, &[4], "4.mkv", &DUB),
            file("Moon Garden", 2, &[1], "s2.mkv", &SUB),
            file("Starlight Academy", 1, &[1], "a.mkv", &SUB),
            file("Starlight Academy", 1, &[1], "b.mkv", &SUB),
            file("Starlight Academy", 1, &[2], "c.mkv", &[]),
        ];
        build(&p, BuildOptions::default())
    }

    #[test]
    fn render_plain_text() {
        let text = render(&sample(), false);
        assert!(!text.contains('\x1b'));
        assert!(text.contains("Moon Garden"));
        assert!(text.contains("S01"));
        assert!(text.contains("missing 3"));
        assert!(text.contains("dupes 1"));
        assert!(text.contains("DD.E"));
        assert!(text.contains("Summary"));
        assert!(text.contains("No English audio: 3 episodes"));
        // worst-first: Starlight Academy (2) before Moon Garden S02 (1)
        let sa = text.find("Starlight Academy S01: 1-2").unwrap();
        let mg = text.find("Moon Garden S02: 1").unwrap();
        assert!(sa < mg);
    }

    #[test]
    fn render_rows_are_aligned() {
        let text = render(&sample(), false);
        let lines: Vec<&str> = text.lines().collect();
        let header = lines[0];
        let map_col = header.find("Map").unwrap();
        for l in &lines[1..4] {
            assert!(l.chars().count() > map_col, "{l}");
        }
        let season_col = header.find("Season").unwrap();
        assert_eq!(&lines[1][season_col..season_col + 3], "S01");
        assert_eq!(&lines[2][season_col..season_col + 3], "S02");
    }

    #[test]
    fn render_with_color_adds_ansi() {
        let text = render(&sample(), true);
        assert!(text.contains("\x1b[32mD"));
        assert!(text.contains("\x1b[0m"));
    }

    #[test]
    fn render_lists_unparsed_and_errors() {
        let mut weird = file("A", 1, &[], "weird.mkv", &[]);
        weird.entry.kind = Kind::Unparsed;
        let mut bad = file("A", 1, &[1], "1.mkv", &[]);
        bad.tracks = Err("could not read".into());
        let text = render(&build(&[weird, bad], BuildOptions::default()), false);
        assert!(text.contains("Unrecognised file names (1)"));
        assert!(text.contains("A/weird.mkv"));
        assert!(text.contains("Files ffprobe could not read (1)"));
        assert!(text.contains("could not read"));
    }

    #[test]
    fn render_empty_and_clean() {
        let empty = build(&[], BuildOptions::default());
        assert!(render(&empty, false).contains("No episodes to show."));
        let clean = build(
            &[file("A", 1, &[1], "1.mkv", &DUAL)],
            BuildOptions::default(),
        );
        assert!(render(&clean, false).contains("Every episode has English audio."));
    }

    #[test]
    fn json_output_has_expected_shape() {
        let v: serde_json::Value = serde_json::to_value(sample()).unwrap();
        assert_eq!(v["summary"]["shows"], 2);
        assert_eq!(v["shows"][0]["name"], "Moon Garden");
        assert_eq!(v["shows"][0]["seasons"][0]["episodes"][0]["status"], "dual");
        assert_eq!(v["no_english"][0]["show"], "Starlight Academy");
    }
}
