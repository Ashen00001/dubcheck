//! ffprobe model: JSON parsing, language classification and per-file audio status.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error(
        "ffprobe not found at '{0}'. Install FFmpeg (e.g. `winget install Gyan.FFmpeg`) and put it on PATH, or pass --ffprobe <path>"
    )]
    NotFound(String),
    #[error("could not run ffprobe: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("ffprobe failed ({code}): {stderr}")]
    Failed { code: String, stderr: String },
    #[error("could not parse ffprobe output: {0}")]
    Json(#[from] serde_json::Error),
}

/// Language of a single audio track, after classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    English,
    Japanese,
    Other,
    Unknown,
}

/// One audio track, reduced to what dubcheck cares about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub lang: Lang,
    pub language_tag: Option<String>,
    pub codec: Option<String>,
    pub title: Option<String>,
    pub default: bool,
    pub commentary: bool,
}

/// Audio summary of a whole file (or the best of several copies of an episode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioStatus {
    /// English and Japanese both present.
    Dual,
    /// English present, no Japanese (a dub-only file).
    English,
    /// Japanese present, no English (sub-only).
    Japanese,
    /// Neither could be identified.
    Unknown,
}

impl AudioStatus {
    pub fn from_tracks(tracks: &[Track]) -> Self {
        let real = tracks.iter().filter(|t| !t.commentary);
        let (mut en, mut ja) = (false, false);
        for t in real {
            match t.lang {
                Lang::English => en = true,
                Lang::Japanese => ja = true,
                _ => {}
            }
        }
        match (en, ja) {
            (true, true) => Self::Dual,
            (true, false) => Self::English,
            (false, true) => Self::Japanese,
            (false, false) => Self::Unknown,
        }
    }

    pub fn has_english(self) -> bool {
        matches!(self, Self::Dual | Self::English)
    }

    /// Higher is better; used to pick the best copy of a duplicated episode.
    pub fn rank(self) -> u8 {
        match self {
            Self::Dual => 3,
            Self::English => 2,
            Self::Japanese => 1,
            Self::Unknown => 0,
        }
    }
}

fn tag_language(tag: &str) -> Option<Lang> {
    match tag.trim().to_ascii_lowercase().replace('_', "-").as_str() {
        "en" | "eng" | "english" | "en-us" | "en-gb" | "en-au" | "en-ca" => Some(Lang::English),
        "ja" | "jpn" | "jp" | "japanese" | "ja-jp" => Some(Lang::Japanese),
        "" | "und" | "undefined" | "unknown" | "mis" | "zxx" => None,
        _ => Some(Lang::Other),
    }
}

fn title_language(title: &str) -> Lang {
    let lower = title.to_ascii_lowercase();
    let mut en = false;
    let mut ja = false;
    for word in lower.split(|c: char| !c.is_ascii_alphabetic()) {
        match word {
            "english" | "eng" | "en" => en = true,
            "japanese" | "jpn" | "jap" | "ja" | "jp" => ja = true,
            _ => {}
        }
    }
    match (en, ja) {
        (true, false) => Lang::English,
        (false, true) => Lang::Japanese,
        _ => Lang::Unknown,
    }
}

/// Decide a track's language from its tag, falling back to its title when the tag is
/// missing or `und`.
pub fn classify_language(tag: Option<&str>, title: Option<&str>) -> Lang {
    if let Some(lang) = tag.and_then(tag_language) {
        return lang;
    }
    title.map_or(Lang::Unknown, title_language)
}

#[derive(Deserialize)]
struct RawOutput {
    #[serde(default)]
    streams: Vec<RawStream>,
}

#[derive(Deserialize)]
struct RawStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    #[serde(default)]
    tags: std::collections::HashMap<String, String>,
    #[serde(default)]
    disposition: std::collections::HashMap<String, i64>,
}

impl RawStream {
    fn tag(&self, name: &str) -> Option<&str> {
        self.tags
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Parse `ffprobe -print_format json -show_streams` output into audio tracks.
pub fn parse_probe_json(json: &str) -> Result<Vec<Track>, ProbeError> {
    let raw: RawOutput = serde_json::from_str(json)?;
    Ok(raw
        .streams
        .iter()
        .filter(|s| s.codec_type.as_deref() == Some("audio"))
        .map(|s| {
            let tag = s.tag("language");
            let title = s.tag("title");
            let commentary = title.is_some_and(|t| t.to_ascii_lowercase().contains("commentary"))
                || s.disposition.get("comment").copied().unwrap_or(0) == 1;
            Track {
                lang: classify_language(tag, title),
                language_tag: tag.map(str::to_owned),
                codec: s.codec_name.clone(),
                title: title.map(str::to_owned),
                default: s.disposition.get("default").copied().unwrap_or(0) == 1,
                commentary,
            }
        })
        .collect())
}

/// Anything that can list the audio tracks of a file. Tests use fakes; the app uses [`Ffprobe`].
pub trait Prober: Sync {
    fn probe(&self, path: &Path) -> Result<Vec<Track>, ProbeError>;
}

/// Runs the real `ffprobe` binary.
#[derive(Debug, Clone)]
pub struct Ffprobe {
    bin: PathBuf,
}

impl Ffprobe {
    /// Find ffprobe (explicit path, else PATH) and check that it runs.
    pub fn locate(explicit: Option<&Path>) -> Result<Self, ProbeError> {
        let bin = explicit.map_or_else(|| PathBuf::from("ffprobe"), Path::to_path_buf);
        match Command::new(&bin).arg("-version").output() {
            Ok(out) if out.status.success() => Ok(Self { bin }),
            Ok(_) => Err(ProbeError::NotFound(bin.display().to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(ProbeError::NotFound(bin.display().to_string()))
            }
            Err(e) => Err(ProbeError::Spawn(e)),
        }
    }
}

impl Prober for Ffprobe {
    fn probe(&self, path: &Path) -> Result<Vec<Track>, ProbeError> {
        let out = Command::new(&self.bin)
            .args(["-v", "quiet", "-print_format", "json", "-show_streams"])
            .arg(path)
            .output()?;
        if !out.status.success() {
            return Err(ProbeError::Failed {
                code: out
                    .status
                    .code()
                    .map_or_else(|| "signal".into(), |c| c.to_string()),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            });
        }
        parse_probe_json(&String::from_utf8_lossy(&out.stdout))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lang(tag: &str, title: &str) -> Lang {
        classify_language(
            (!tag.is_empty()).then_some(tag),
            (!title.is_empty()).then_some(title),
        )
    }

    #[test]
    fn english_tags() {
        for t in ["eng", "en", "English", "ENG", "en-US", "en_GB"] {
            assert_eq!(lang(t, ""), Lang::English, "{t}");
        }
    }

    #[test]
    fn japanese_tags() {
        for t in ["jpn", "ja", "Japanese", "JPN", "ja-JP", "jp"] {
            assert_eq!(lang(t, ""), Lang::Japanese, "{t}");
        }
    }

    #[test]
    fn und_uses_title() {
        assert_eq!(lang("und", "English"), Lang::English);
        assert_eq!(lang("und", "English 5.1 DDP"), Lang::English);
        assert_eq!(lang("und", "Japanese (Stereo)"), Lang::Japanese);
        assert_eq!(lang("", "JPN"), Lang::Japanese);
    }

    #[test]
    fn und_without_helpful_title_is_unknown() {
        assert_eq!(lang("und", ""), Lang::Unknown);
        assert_eq!(lang("", ""), Lang::Unknown);
        assert_eq!(lang("und", "Surround 5.1"), Lang::Unknown);
        assert_eq!(lang("und", "English / Japanese"), Lang::Unknown);
    }

    #[test]
    fn tag_beats_title() {
        assert_eq!(lang("jpn", "English"), Lang::Japanese);
    }

    #[test]
    fn other_languages() {
        assert_eq!(lang("fre", ""), Lang::Other);
        assert_eq!(lang("spa", "English"), Lang::Other);
    }

    #[test]
    fn title_words_need_to_be_whole_words() {
        assert_eq!(lang("und", "Engine"), Lang::Unknown);
        assert_eq!(lang("und", "Jasper"), Lang::Unknown);
    }

    const DUAL: &str = r#"{"streams":[
        {"index":0,"codec_type":"video","codec_name":"hevc"},
        {"index":1,"codec_type":"audio","codec_name":"aac","disposition":{"default":1},
         "tags":{"language":"jpn","title":"Japanese"}},
        {"index":2,"codec_type":"audio","codec_name":"ac3","disposition":{"default":0},
         "tags":{"language":"eng","title":"English"}},
        {"index":3,"codec_type":"subtitle","codec_name":"ass","tags":{"language":"eng"}}
    ]}"#;

    #[test]
    fn parses_dual_audio() {
        let tracks = parse_probe_json(DUAL).unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].lang, Lang::Japanese);
        assert!(tracks[0].default);
        assert_eq!(tracks[0].codec.as_deref(), Some("aac"));
        assert_eq!(tracks[1].lang, Lang::English);
        assert!(!tracks[1].default);
        assert_eq!(AudioStatus::from_tracks(&tracks), AudioStatus::Dual);
    }

    #[test]
    fn subtitles_do_not_count_as_audio() {
        let json = r#"{"streams":[
            {"codec_type":"audio","codec_name":"aac","tags":{"language":"jpn"}},
            {"codec_type":"subtitle","codec_name":"ass","tags":{"language":"eng"}}]}"#;
        let tracks = parse_probe_json(json).unwrap();
        assert_eq!(AudioStatus::from_tracks(&tracks), AudioStatus::Japanese);
    }

    #[test]
    fn english_only_file() {
        let json = r#"{"streams":[{"codec_type":"audio","tags":{"language":"eng"}}]}"#;
        let tracks = parse_probe_json(json).unwrap();
        assert_eq!(AudioStatus::from_tracks(&tracks), AudioStatus::English);
    }

    #[test]
    fn und_track_titled_english_counts() {
        let json = r#"{"streams":[
            {"codec_type":"audio","tags":{"language":"jpn"}},
            {"codec_type":"audio","tags":{"language":"und","title":"English"}}]}"#;
        let tracks = parse_probe_json(json).unwrap();
        assert_eq!(AudioStatus::from_tracks(&tracks), AudioStatus::Dual);
    }

    #[test]
    fn tag_keys_are_case_insensitive() {
        let json =
            r#"{"streams":[{"codec_type":"audio","tags":{"LANGUAGE":"eng","TITLE":"Dub"}}]}"#;
        let tracks = parse_probe_json(json).unwrap();
        assert_eq!(tracks[0].lang, Lang::English);
        assert_eq!(tracks[0].title.as_deref(), Some("Dub"));
    }

    #[test]
    fn commentary_tracks_are_ignored() {
        let json = r#"{"streams":[
            {"codec_type":"audio","tags":{"language":"jpn"}},
            {"codec_type":"audio","tags":{"language":"eng","title":"Director Commentary"}}]}"#;
        let tracks = parse_probe_json(json).unwrap();
        assert!(tracks[1].commentary);
        assert_eq!(AudioStatus::from_tracks(&tracks), AudioStatus::Japanese);
    }

    #[test]
    fn commentary_disposition_flag() {
        let json = r#"{"streams":[
            {"codec_type":"audio","tags":{"language":"eng"},"disposition":{"comment":1}}]}"#;
        let tracks = parse_probe_json(json).unwrap();
        assert_eq!(AudioStatus::from_tracks(&tracks), AudioStatus::Unknown);
    }

    #[test]
    fn untagged_audio_is_unknown() {
        let json = r#"{"streams":[{"codec_type":"audio","codec_name":"flac"}]}"#;
        let tracks = parse_probe_json(json).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(AudioStatus::from_tracks(&tracks), AudioStatus::Unknown);
    }

    #[test]
    fn no_streams_is_unknown() {
        assert_eq!(
            AudioStatus::from_tracks(&parse_probe_json("{}").unwrap()),
            AudioStatus::Unknown
        );
        assert_eq!(
            AudioStatus::from_tracks(&parse_probe_json(r#"{"streams":[]}"#).unwrap()),
            AudioStatus::Unknown
        );
    }

    #[test]
    fn bad_json_is_an_error() {
        assert!(matches!(
            parse_probe_json("not json"),
            Err(ProbeError::Json(_))
        ));
    }

    #[test]
    fn status_helpers() {
        assert!(AudioStatus::Dual.has_english());
        assert!(AudioStatus::English.has_english());
        assert!(!AudioStatus::Japanese.has_english());
        assert!(AudioStatus::Dual.rank() > AudioStatus::English.rank());
        assert!(AudioStatus::English.rank() > AudioStatus::Japanese.rank());
        assert!(AudioStatus::Japanese.rank() > AudioStatus::Unknown.rank());
    }

    #[test]
    fn missing_ffprobe_gives_clear_error() {
        let err =
            Ffprobe::locate(Some(Path::new("definitely-not-a-real-ffprobe-binary"))).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("ffprobe not found"), "{msg}");
        assert!(msg.contains("--ffprobe"), "{msg}");
    }
}
