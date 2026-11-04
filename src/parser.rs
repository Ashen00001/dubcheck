//! Filename and folder parsing: turns anime-style names into season/episode numbers.

use regex::Regex;
use std::sync::LazyLock;

/// What the parser made of a file name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// One or more episodes. `season` is only set when the name itself says so.
    Episode {
        season: Option<u32>,
        episodes: Vec<u32>,
    },
    /// Openings, endings, trailers, samples and similar. Not an episode, not an error.
    Extra,
    /// Nothing recognisable.
    Unknown,
}

const RESOLUTIONS: [u32; 5] = [480, 576, 720, 1080, 2160];

static EXTRA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:^|[^a-z0-9])(?:(?:ncop|nced)\d{0,2}|creditless|trailer|sample|promo|(?:op|ed)\d{1,2}|clean[ ._-]?(?:op|ed|opening|ending))(?:[^a-z0-9]|$)",
    )
    .unwrap()
});
static SXE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])s(\d{1,2})[ ._-]?e(\d{1,4})").unwrap());
static CONT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:([ ._]?-[ ._]?e?)|([ ._]?e))(\d{1,4})").unwrap());
static NXN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|[^0-9a-z])(\d{1,2})x(\d{1,3})(?:[^0-9a-z]|$)").unwrap());
static SPECIAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(?:ova|oad|oav|ona|special|sp)[ ._-]?(\d{1,3})(?:[^0-9a-z]|$)")
        .unwrap()
});
static EP_WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(?:episode|ep|e)[ ._-]?(\d{1,4})(?:v\d+)?(?:[^0-9a-z]|$)")
        .unwrap()
});
static DASH_NUM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|\s)-\s*(\d{1,4})(?:v\d+)?(?:\s|$)").unwrap());
static BRACKET_NUM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[(\d{1,4})(?:v\d+)?\]").unwrap());
static BRACKETS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[[^\]]*\]|\([^)]*\)").unwrap());
static TECH_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:\d{3,4}p|\d{3,4}x\d{3,4}|x26[45]|h26[45]|hevc|avc|aac|flac|ac3|opus|10bit|8bit|web(?:-?dl|rip)?|bluray|bd(?:rip)?|dual|multi|dub|sub|eng|jpn|av1|hi10p?|hdr)$",
    )
    .unwrap()
});
static TRAILING_NUM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(\d{1,4})(?:v\d+)?$").unwrap());
static SEASON_DIR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:season|series|s)[ ._-]?(\d{1,2})(?:[^0-9].*)?$").unwrap()
});
static SPECIALS_DIR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:specials?|ovas?|oads?)$").unwrap());

/// Season number implied by a folder name (`Season 02`, `S2`, `Specials` is 0).
pub fn parse_season_folder(name: &str) -> Option<u32> {
    let name = name.trim();
    if SPECIALS_DIR.is_match(name) {
        return Some(0);
    }
    SEASON_DIR
        .captures(name)
        .and_then(|c| c[1].parse::<u32>().ok())
}

fn is_year(n: u32) -> bool {
    (1900..=2099).contains(&n)
}

/// Follow-on episodes after an `SxxEyy` match: `-E02`, `-02`, `E02`.
fn continuation(mut rest: &str, first: u32) -> Vec<u32> {
    let mut eps = vec![first];
    let mut last = first;
    while let Some(c) = CONT.captures(rest) {
        let whole = c.get(0).unwrap();
        let Ok(n) = c[3].parse::<u32>() else { break };
        let next_char = rest[whole.end()..].chars().next();
        if next_char.is_some_and(|ch| ch.is_alphabetic() && !matches!(ch, 'e' | 'E')) {
            break;
        }
        if c.get(1).is_some() {
            if RESOLUTIONS.contains(&n) || n <= last || n - last > 30 {
                break;
            }
            eps.extend(last + 1..=n);
        } else {
            eps.push(n);
        }
        last = n;
        rest = &rest[whole.end()..];
    }
    eps
}

fn clean(stem: &str) -> String {
    let no_brackets = BRACKETS.replace_all(stem, " ");
    let spaced: String = no_brackets
        .chars()
        .map(|c| if c == '_' || c == '.' { ' ' } else { c })
        .collect();
    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse a file stem (no extension) into a [`Parsed`].
pub fn parse_episode(stem: &str) -> Parsed {
    if EXTRA.is_match(stem) {
        return Parsed::Extra;
    }

    if let Some(c) = SXE.captures(stem) {
        if let (Ok(s), Ok(e)) = (c[1].parse::<u32>(), c[2].parse::<u32>()) {
            let end = c.get(0).unwrap().end();
            return Parsed::Episode {
                season: Some(s),
                episodes: continuation(&stem[end..], e),
            };
        }
    }

    if let Some(c) = NXN.captures(stem) {
        if let (Ok(s), Ok(e)) = (c[1].parse::<u32>(), c[2].parse::<u32>()) {
            return Parsed::Episode {
                season: Some(s),
                episodes: vec![e],
            };
        }
    }

    if let Some(c) = SPECIAL.captures(stem) {
        if let Ok(e) = c[1].parse::<u32>() {
            return Parsed::Episode {
                season: Some(0),
                episodes: vec![e],
            };
        }
    }

    if let Some(c) = EP_WORD.captures(stem) {
        if let Ok(e) = c[1].parse::<u32>() {
            return Parsed::Episode {
                season: None,
                episodes: vec![e],
            };
        }
    }

    let cleaned = clean(stem);
    let mut dash = None;
    for c in DASH_NUM.captures_iter(&cleaned) {
        if let Ok(n) = c[1].parse::<u32>() {
            if !RESOLUTIONS.contains(&n) && !is_year(n) {
                dash = Some(n);
            }
        }
    }
    if let Some(e) = dash {
        return Parsed::Episode {
            season: None,
            episodes: vec![e],
        };
    }

    if let Some(c) = BRACKET_NUM.captures_iter(stem).last() {
        if let Ok(e) = c[1].parse::<u32>() {
            if !is_year(e) && !RESOLUTIONS.contains(&e) {
                return Parsed::Episode {
                    season: None,
                    episodes: vec![e],
                };
            }
        }
    }

    let tokens: Vec<&str> = cleaned.split(' ').collect();
    for (i, token) in tokens.iter().enumerate().rev() {
        if TECH_TOKEN.is_match(token) {
            continue;
        }
        // "H.264" / "x.265" end up as two tokens after cleaning
        if i > 0 && matches!(tokens[i - 1].to_ascii_lowercase().as_str(), "h" | "x") {
            continue;
        }
        if let Some(c) = TRAILING_NUM.captures(token) {
            if let Ok(e) = c[1].parse::<u32>() {
                if !is_year(e) && !RESOLUTIONS.contains(&e) {
                    return Parsed::Episode {
                        season: None,
                        episodes: vec![e],
                    };
                }
            }
        }
        break;
    }

    Parsed::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(season: Option<u32>, episodes: &[u32]) -> Parsed {
        Parsed::Episode {
            season,
            episodes: episodes.to_vec(),
        }
    }

    #[test]
    fn standard_sxxexx() {
        assert_eq!(parse_episode("Moon Garden - S01E01"), ep(Some(1), &[1]));
        assert_eq!(
            parse_episode("moon.garden.s02e13.1080p"),
            ep(Some(2), &[13])
        );
        assert_eq!(parse_episode("Show S1E5"), ep(Some(1), &[5]));
        assert_eq!(parse_episode("Show - s01 e07 - Title"), ep(Some(1), &[7]));
    }

    #[test]
    fn three_digit_episodes() {
        assert_eq!(parse_episode("Pirate Sea - S01E1021"), ep(Some(1), &[1021]));
        assert_eq!(parse_episode("Pirate Sea S01E100"), ep(Some(1), &[100]));
    }

    #[test]
    fn multi_episode_files() {
        assert_eq!(parse_episode("Show - S01E01-E02"), ep(Some(1), &[1, 2]));
        assert_eq!(parse_episode("Show - S01E01E02"), ep(Some(1), &[1, 2]));
        assert_eq!(parse_episode("Show - S01E03-05"), ep(Some(1), &[3, 4, 5]));
        assert_eq!(parse_episode("Show S01E01-E03"), ep(Some(1), &[1, 2, 3]));
    }

    #[test]
    fn resolution_after_dash_is_not_a_range() {
        assert_eq!(parse_episode("Show S01E04-1080p"), ep(Some(1), &[4]));
    }

    #[test]
    fn title_after_dash_is_not_a_range() {
        assert_eq!(
            parse_episode("Show - S01E04 - The Long Night"),
            ep(Some(1), &[4])
        );
    }

    #[test]
    fn nxn_style() {
        assert_eq!(parse_episode("Show 1x05"), ep(Some(1), &[5]));
        assert_eq!(parse_episode("Show - 2x13 - Title"), ep(Some(2), &[13]));
    }

    #[test]
    fn resolution_is_not_nxn() {
        assert_eq!(parse_episode("Show - 05 1920x1080"), ep(None, &[5]));
    }

    #[test]
    fn fansub_dash_number() {
        assert_eq!(
            parse_episode("[Fake-Subs] Starlight Academy - 05 [1080p][ABCD1234]"),
            ep(None, &[5])
        );
        assert_eq!(
            parse_episode("[Fake-Subs] Starlight Academy - 12v2 [720p]"),
            ep(None, &[12])
        );
        assert_eq!(
            parse_episode("[Grp] Starlight Academy - 105 (BD 1080p HEVC)"),
            ep(None, &[105])
        );
    }

    #[test]
    fn dotted_and_underscored() {
        assert_eq!(
            parse_episode("Starlight_Academy_-_07_[720p]"),
            ep(None, &[7])
        );
        assert_eq!(
            parse_episode("Starlight.Academy.-.08.1080p.WEB"),
            ep(None, &[8])
        );
    }

    #[test]
    fn episode_words() {
        assert_eq!(parse_episode("Show Episode 9"), ep(None, &[9]));
        assert_eq!(parse_episode("Show Ep.10"), ep(None, &[10]));
        assert_eq!(parse_episode("Show E11"), ep(None, &[11]));
    }

    #[test]
    fn bare_trailing_number() {
        assert_eq!(parse_episode("Starlight Academy 05"), ep(None, &[5]));
        assert_eq!(
            parse_episode("Starlight Academy 05 1080p x265"),
            ep(None, &[5])
        );
        assert_eq!(parse_episode("05"), ep(None, &[5]));
    }

    #[test]
    fn bracketed_number() {
        assert_eq!(
            parse_episode("[Grp] Starlight Academy [06] [1080p]"),
            ep(None, &[6])
        );
    }

    #[test]
    fn specials() {
        assert_eq!(parse_episode("Show - S00E03"), ep(Some(0), &[3]));
        assert_eq!(parse_episode("Show - OVA 2"), ep(Some(0), &[2]));
        assert_eq!(parse_episode("Show OAD01"), ep(Some(0), &[1]));
        assert_eq!(parse_episode("Show - Special 3"), ep(Some(0), &[3]));
        assert_eq!(
            parse_episode("[Grp] Show - SP01 [1080p]"),
            ep(Some(0), &[1])
        );
    }

    #[test]
    fn extras_are_not_episodes() {
        assert_eq!(parse_episode("Show - NCOP1"), Parsed::Extra);
        assert_eq!(parse_episode("Show - NCED"), Parsed::Extra);
        assert_eq!(parse_episode("Show - Creditless Opening"), Parsed::Extra);
        assert_eq!(parse_episode("Show - OP2"), Parsed::Extra);
        assert_eq!(parse_episode("Show S01E03 sample"), Parsed::Extra);
        assert_eq!(parse_episode("Show-trailer"), Parsed::Extra);
    }

    #[test]
    fn years_and_resolutions_are_not_episodes() {
        assert_eq!(parse_episode("Starlight Academy (2019)"), Parsed::Unknown);
        assert_eq!(parse_episode("Starlight Academy 2019"), Parsed::Unknown);
        assert_eq!(parse_episode("Starlight Academy 1080p"), Parsed::Unknown);
    }

    #[test]
    fn unparseable_names() {
        assert_eq!(parse_episode("Starlight Academy"), Parsed::Unknown);
        assert_eq!(parse_episode("movie"), Parsed::Unknown);
        assert_eq!(parse_episode(""), Parsed::Unknown);
    }

    #[test]
    fn x265_is_not_an_episode() {
        assert_eq!(parse_episode("Show x265 AAC"), Parsed::Unknown);
        assert_eq!(parse_episode("Show H.264"), Parsed::Unknown);
    }

    #[test]
    fn season_folders() {
        assert_eq!(parse_season_folder("Season 01"), Some(1));
        assert_eq!(parse_season_folder("Season 2"), Some(2));
        assert_eq!(parse_season_folder("season_03"), Some(3));
        assert_eq!(parse_season_folder("S04"), Some(4));
        assert_eq!(parse_season_folder("Series 2"), Some(2));
        assert_eq!(parse_season_folder("Season 2 (2020)"), Some(2));
        assert_eq!(parse_season_folder("Season 00"), Some(0));
        assert_eq!(parse_season_folder("Specials"), Some(0));
        assert_eq!(parse_season_folder("OVA"), Some(0));
    }

    #[test]
    fn non_season_folders() {
        assert_eq!(parse_season_folder("Starlight Academy"), None);
        assert_eq!(parse_season_folder("Sword Dance"), None);
        assert_eq!(parse_season_folder("Seasonal Specials Pack"), None);
    }
}
