# dubcheck

A small command-line tool that audits an anime library for audio languages. It tells you, per show and per season, which episodes have English audio, Japanese audio, both (dual audio) or neither. It also flags missing episodes, duplicate copies and file names it can't parse.

I made it for my own Jellyfin library, where the rule is "dub or nothing, dual audio if possible". It's written in Rust and is **read-only**: it never renames, moves, deletes or writes anything inside the library folder.

Status: works, and I've run it on a real library of a few thousand episodes. Still early (v0.1), see [Limitations](#limitations).

## What it reports

- **Audio per episode**: dual (English + Japanese), English only, Japanese only, or unknown.
- **Missing episodes** in a season's sequence (e.g. you have 1-10 and 12).
- **Duplicates**: more than one file for the same season/episode.
- **Unrecognised file names** the parser couldn't turn into an episode.
- **Summary** with counts, plus a list of seasons with no English audio, worst first.

## Install / build

You need [Rust](https://rustup.rs) and [FFmpeg](https://ffmpeg.org) (for `ffprobe`).

```
git clone https://github.com/Ashen00001/dubcheck
cd dubcheck
cargo build --release
```

The binary ends up in `target/release/dubcheck` (`dubcheck.exe` on Windows). On Windows, `winget install Gyan.FFmpeg` gets you ffprobe.

dubcheck looks for `ffprobe` on `PATH`. If it isn't there, pass `--ffprobe <path>`. If it can't be found you get a clear error and nothing else runs.

## Usage

```
dubcheck <LIBRARY_PATH> [options]
```

| Flag | What it does |
| --- | --- |
| `--json` | Print the report as JSON instead of a table |
| `--only-missing-dub` | Only list seasons that have episodes without English audio |
| `--show <text>` | Only scan shows whose folder name contains the text (case-insensitive) |
| `--no-cache` | Don't read or write the probe cache |
| `--cache-file <file>` | Use a specific cache file |
| `--ffprobe <path>` | Path to the ffprobe binary |
| `-j, --jobs <n>` | Files to probe in parallel (default 4) |
| `--lenient-start` | Only look for gaps between the first and last episode present |

Colors are used when stdout is a terminal. Set `NO_COLOR` to turn them off.

### Exit codes

The exit code is a bitmask, so scripts can tell what kind of problem there was:

| Code | Meaning |
| --- | --- |
| 0 | Everything is fine |
| 1 | Error (bad path, ffprobe missing, ...) |
| +2 | Structural problems: missing episodes, duplicates, unrecognised or unreadable files |
| +4 | At least one episode has no English audio |

So `6` means both kinds of problem.

### Sample output

All names here are made up.

```
Show               Season     Eps  Dual   Eng   Jpn    ?  Notes                Map
Moon Garden        S01         11    11     0     0    0  missing 11; dupes 5  DDDDDDDDDD.D
                   S02          2     0     0     2    0                       JJ
Paper Lanterns     S01          3     0     3     0    0                       EEE
Starlight Academy  S01          6     0     0     6    0                       JJJJJJ

Map: D dual, E english only, J japanese only, ? unknown, . missing

Unrecognised file names (1)
  Starlight Academy/readme-extra.mkv

Summary
  3 shows, 4 seasons, 22 episodes in 23 files
  dual 11  english-only 3  japanese-only 8  unknown 0
  missing 1  duplicates 1  unrecognised 1  unreadable 0

No English audio: 8 episodes (worst first)
     6 of 6    Starlight Academy S01: 1-6
     2 of 2    Moon Garden S02: 1-2
```

The `Map` column has one character per episode slot, so you can see the shape of a season at a glance.

## How it works

1. **Walk** the library folder (read-only). The expected layout is `Show Name/Season 01/Show Name - S01E01.mkv`, but it copes with a lot of anime naming: `[Group] Show - 05 [1080p]`, `Show - 12v2`, `1x05`, `Episode 5`, multi-episode files (`S01E01-E02`), `Specials`/`OVA` (season 0), and so on. Openings/endings (NCOP/NCED), trailers, samples and `Extras` folders are skipped. The season comes from the file name if it has one, otherwise from a `Season N` folder, otherwise it's assumed to be season 1.
2. **Probe** each episode with `ffprobe -v quiet -print_format json -show_streams` and read the audio streams. Probing runs in parallel (rayon).
3. **Classify** each audio track's language (below), then each file as dual / English / Japanese / unknown.
4. **Report**: group by show and season, find gaps and duplicates, print the table.

### Language detection

- Tags `eng`, `en`, `english` (and `en-US` style variants) count as English; `jpn`, `ja`, `jp`, `japanese` count as Japanese.
- If the tag is missing or `und`, the track **title** is used: a track called "English" or "Japanese (Stereo)" is classified from that. If the title mentions both languages or neither, it stays unknown.
- If the tag names some other language (say `fre`), the track is "other" and counts as neither.
- The tag wins over the title when both are set and disagree.
- Commentary tracks (title contains "commentary", or the commentary disposition flag) are ignored.
- Subtitle streams are ignored; only audio counts.
- A file with an English track plus an untagged track is reported as English only, since the untagged one could be anything.

### Cache

ffprobe on a big library on a hard drive is slow (about 3,800 episodes took roughly 10 minutes the first time with 2 workers). Results are cached in a JSON file keyed by path, size and modified time, so a rescan only probes new or changed files and takes a fraction of a second when nothing changed. The cache lives in your user cache folder (`%LOCALAPPDATA%\dubcheck` on Windows, `~/.cache/dubcheck` elsewhere) unless you set `--cache-file`. dubcheck refuses to put the cache inside the library.

## Limitations

- It trusts the language tags in the files. A Japanese track tagged `eng` will be reported wrong, and untagged files show up as unknown.
- It only checks that an English audio track exists, not that it's a good one.
- Gap detection assumes episodes start at 1 in every season. Libraries that number split cours continuously (a season starting at 13) will show "missing 1-12". Use `--lenient-start` for those.
- Absolute-numbered shows with no season folders are treated as season 1.
- Filename parsing is heuristic. It handles a lot of patterns (there are plenty of tests) but not everything; anything it can't read is listed as unrecognised rather than guessed.
- No progress output during a long first scan.
- The cache isn't pruned of files that have been deleted.

## Roadmap

- Progress output for long scans
- Prune stale cache entries
- Expected episode counts (from NFO files) so missing trailing episodes get caught
- Optional CSV output
- Audio codec/channel info in the report (already parsed, just not shown)
- Release binaries

## Optional: run it in the background

[`contrib/run-dubcheck.ps1`](contrib/run-dubcheck.ps1) is a Windows wrapper for running dubcheck unattended, for example from a startup script or a scheduled task. This part is entirely optional.

1. Put `dubcheck.exe` and the script in the same folder.
2. Run `powershell -File run-dubcheck.ps1 -Library <your library>`.

It runs at below-normal priority with a low `--jobs` value, skips itself if a run finished in the last 6 hours or one is already going (lock file), writes `latest.txt` and `latest.json` next to the script, and keeps a small rolling `dubcheck.log`. It swallows its own errors and always exits 0, so it can't break whatever launched it.

## Development

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Tests don't need ffprobe or any media files: they use hand-written ffprobe JSON and empty files in temp folders. CI runs them on Ubuntu and Windows.

## License

MIT, see [LICENSE](LICENSE).
