//! File name patterns, using the same tokens as ShareX:
//!
//! | token | value |
//! |---|---|
//! | `%y` `%yy` | year (2026, 26) |
//! | `%mo` `%mon` | month (01, Oct) |
//! | `%d` | day of month |
//! | `%h` `%mi` `%s` `%ms` | hour, minute, second, millisecond |
//! | `%pm` | AM or PM |
//! | `%unix` | Unix time in seconds |
//! | `%rn` | random digit, `%ra` random letter or digit |
//! | `%width` `%height` | image size |
//! | `%t` | window title (window captures) |
//!
//! `/` (or `\`) in a pattern starts a subfolder.

use std::path::{Path, PathBuf};

use chrono::{Datelike, Local, NaiveDateTime, Timelike};

#[derive(Clone, Debug, Default)]
pub struct NameContext {
    pub width: u32,
    pub height: u32,
    pub title: String,
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// Tokens longest first, so `%mon` wins over `%mo` and `%ms` over `%m...`.
const TOKENS: [&str; 15] =
    ["%height", "%width", "%unix", "%mon", "%yy", "%mo", "%mi", "%ms", "%pm", "%rn", "%ra", "%y", "%d", "%h", "%s"];

pub fn expand(pattern: &str, ctx: &NameContext) -> String {
    expand_at(pattern, ctx, Local::now().naive_local())
}

pub fn expand_at(pattern: &str, ctx: &NameContext, now: NaiveDateTime) -> String {
    let mut out = String::with_capacity(pattern.len() + 16);
    let mut rest = pattern;
    'outer: while !rest.is_empty() {
        if rest.starts_with('%') {
            for token in TOKENS {
                if let Some(after) = rest.strip_prefix(token) {
                    out.push_str(&value(token, ctx, now));
                    rest = after;
                    continue 'outer;
                }
            }
            if let Some(after) = rest.strip_prefix("%t") {
                out.push_str(&sanitize_component(&ctx.title));
                rest = after;
                continue;
            }
        }
        let c = rest.chars().next().unwrap();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

fn value(token: &str, ctx: &NameContext, now: NaiveDateTime) -> String {
    match token {
        "%y" => format!("{:04}", now.year()),
        "%yy" => format!("{:02}", now.year() % 100),
        "%mo" => format!("{:02}", now.month()),
        "%mon" => MONTHS[now.month0() as usize].to_string(),
        "%d" => format!("{:02}", now.day()),
        "%h" => format!("{:02}", now.hour()),
        "%mi" => format!("{:02}", now.minute()),
        "%s" => format!("{:02}", now.second()),
        "%ms" => format!("{:03}", now.and_utc().timestamp_subsec_millis()),
        "%pm" => if now.hour() < 12 { "AM" } else { "PM" }.to_string(),
        "%unix" => now.and_utc().timestamp().to_string(),
        "%rn" => fastrand::u8(0..10).to_string(),
        "%ra" => {
            const ALNUM: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
            (ALNUM[fastrand::usize(..ALNUM.len())] as char).to_string()
        }
        "%width" => ctx.width.to_string(),
        "%height" => ctx.height.to_string(),
        _ => String::new(),
    }
}

/// Removes characters that aren't allowed in file names on any of the three systems.
pub fn sanitize_component(s: &str) -> String {
    let cleaned: String =
        s.chars()
            .map(|c| {
                if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() {
                    '_'
                } else {
                    c
                }
            })
            .collect();
    let trimmed = cleaned.trim().trim_end_matches('.');
    let limited: String = trimmed.chars().take(120).collect();
    limited
}

/// Joins an expanded pattern onto `dir`, treating `/` and `\` as folder separators and cleaning
/// each part. Never escapes `dir`: `..` parts are dropped.
pub fn build_path(dir: &Path, expanded: &str, extension: &str) -> PathBuf {
    let mut path = dir.to_path_buf();
    let parts: Vec<String> = expanded
        .split(['/', '\\'])
        .map(sanitize_component)
        .filter(|p| !p.is_empty() && p != "." && p != "..")
        .collect();
    let (last, folders) = match parts.split_last() {
        Some((last, folders)) => (last.clone(), folders.to_vec()),
        None => ("Snapsmith".to_string(), Vec::new()),
    };
    for f in folders {
        path.push(f);
    }
    path.push(format!("{last}.{extension}"));
    path
}

/// `path` if nothing is there yet, otherwise `name (2).ext`, `name (3).ext`, ...
pub fn unique(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|s| s.to_string_lossy().into_owned());
    for n in 2.. {
        let name = match &ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = path.with_file_name(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn at() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 3, 7).unwrap().and_hms_milli_opt(14, 5, 9, 42).unwrap()
    }

    #[test]
    fn expands_date_tokens() {
        let ctx = NameContext { width: 800, height: 600, title: "Notes: a/b".into() };
        assert_eq!(
            expand_at("%y-%mo/Shot_%y-%mo-%d_%h-%mi-%s.%ms", &ctx, at()),
            "2026-03/Shot_2026-03-07_14-05-09.042"
        );
        assert_eq!(expand_at("%yy %mon %pm %width x %height", &ctx, at()), "26 Mar PM 800 x 600");
        assert_eq!(expand_at("%t", &ctx, at()), "Notes_ a_b");
    }

    #[test]
    fn unknown_tokens_stay() {
        assert_eq!(expand_at("100%_%q", &NameContext::default(), at()), "100%_%q");
    }

    #[test]
    fn random_tokens_have_the_right_shape() {
        let s = expand_at("%rn%rn%ra", &NameContext::default(), at());
        assert_eq!(s.len(), 3);
        assert!(s[..2].chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn build_path_makes_subfolders_and_stays_inside() {
        let dir = Path::new("base");
        let p = build_path(dir, "2026-03/../Shot: 1", "png");
        assert_eq!(p, Path::new("base").join("2026-03").join("Shot_ 1.png"));
        assert_eq!(build_path(dir, "", "jpg"), Path::new("base").join("Snapsmith.jpg"));
    }

    #[test]
    fn unique_appends_a_counter() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.png");
        assert_eq!(unique(p.clone()), p);
        std::fs::write(&p, b"x").unwrap();
        assert_eq!(unique(p.clone()), dir.path().join("a (2).png"));
    }
}
