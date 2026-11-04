//! Command-line interface and the glue between the modules.

use crate::cache::Cache;
use crate::probe::{Ffprobe, Prober};
use crate::report::{self, BuildOptions, Report};
use crate::scanner;
use anyhow::{bail, Context, Result};
use clap::Parser;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

/// Read-only audit of an anime library: English/Japanese audio, missing episodes, duplicates.
///
/// Exit code: 0 = clean, 1 = error, +2 = gaps/duplicates/unreadable files,
/// +4 = episodes without English audio (so 6 means both).
#[derive(Parser, Debug)]
#[command(name = "dubcheck", version)]
pub struct Cli {
    /// Library folder (shows laid out as `Show/Season 01/Show - S01E01.mkv`)
    pub path: PathBuf,

    /// Print the report as JSON
    #[arg(long)]
    pub json: bool,

    /// Only list seasons that have episodes without English audio
    #[arg(long)]
    pub only_missing_dub: bool,

    /// Only scan shows whose folder name contains this text (case-insensitive)
    #[arg(long, value_name = "TEXT")]
    pub show: Option<String>,

    /// Ignore and do not write the probe cache
    #[arg(long)]
    pub no_cache: bool,

    /// Where to keep the probe cache (default: a per-user cache folder)
    #[arg(long, value_name = "FILE")]
    pub cache_file: Option<PathBuf>,

    /// Path to the ffprobe binary (default: look on PATH)
    #[arg(long, value_name = "PATH")]
    pub ffprobe: Option<PathBuf>,

    /// Number of files to probe in parallel
    #[arg(long, short = 'j', default_value_t = 4)]
    pub jobs: usize,

    /// Only look for missing episodes between the first and last one present
    #[arg(long)]
    pub lenient_start: bool,
}

/// Default cache location: `%LOCALAPPDATA%\dubcheck\cache.json`, `$XDG_CACHE_HOME/dubcheck/…`
/// or `~/.cache/dubcheck/…`, falling back to the current folder.
pub fn default_cache_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_CACHE_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")));
    base.map_or_else(
        || PathBuf::from("dubcheck-cache.json"),
        |b| b.join("dubcheck").join("cache.json"),
    )
}

/// Refuse to put the cache inside the library: dubcheck never writes there.
fn ensure_cache_outside(root: &Path, cache: &Path) -> Result<()> {
    let abs = std::path::absolute(cache).unwrap_or_else(|_| cache.to_path_buf());
    let parent = abs.parent().unwrap_or(&abs);
    // the cache folder may not exist yet; walk up to the nearest one that does
    let existing = parent.ancestors().find(|p| p.exists());
    let Some(existing) = existing.and_then(|p| p.canonicalize().ok()) else {
        return Ok(());
    };
    if existing.starts_with(root) {
        bail!(
            "the cache file ({}) is inside the library. dubcheck never writes to the library; use --cache-file or --no-cache",
            cache.display()
        );
    }
    Ok(())
}

/// Scan, probe and build the report. Separate from `run` so it can be tested with a fake prober.
pub fn scan<P: Prober>(
    root: &Path,
    cli: &Cli,
    prober: &P,
    cache: &mut Cache,
) -> Result<(Report, scanner::ProbeStats)> {
    let found = scanner::discover(root, cli.show.as_deref())?;
    for w in &found.warnings {
        eprintln!("warning: {w}");
    }
    let (probed, stats) = scanner::probe_all(found.files, prober, cache, cli.jobs)?;
    let mut rep = report::build(
        &probed,
        BuildOptions {
            lenient_start: cli.lenient_start,
        },
    );
    if cli.only_missing_dub {
        rep.retain_missing_dub();
    }
    Ok((rep, stats))
}

pub fn run(cli: &Cli) -> Result<u8> {
    let root = cli
        .path
        .canonicalize()
        .with_context(|| format!("cannot open '{}'", cli.path.display()))?;
    anyhow::ensure!(root.is_dir(), "'{}' is not a directory", cli.path.display());

    let ffprobe = Ffprobe::locate(cli.ffprobe.as_deref())?;

    let mut cache = if cli.no_cache {
        Cache::disabled()
    } else {
        let path = cli.cache_file.clone().unwrap_or_else(default_cache_path);
        ensure_cache_outside(&root, &path)?;
        Cache::load(&path)
    };

    let (rep, stats) = scan(&root, cli, &ffprobe, &mut cache)?;

    if let Err(e) = cache.save() {
        eprintln!("warning: could not save cache: {e}");
    }

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&rep)?);
    } else {
        let color = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
        print!("{}", report::render(&rep, color));
        if std::io::stderr().is_terminal() {
            eprintln!(
                "\n({} files probed, {} from cache)",
                stats.probed, stats.cached
            );
        }
    }
    Ok(rep.exit_code())
}
