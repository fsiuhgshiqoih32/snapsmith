//! ShareX custom-uploader template syntax.
//!
//! Current syntax (ShareX 13 and later): `{name}` or `{name:arg|arg}`, nestable, with `\` escaping
//! `{ } | \`. Supported functions:
//!
//! - `{response}`, `{responseurl}`, `{header:Name}`
//! - `{json:path}` or `{json:source|path}`: `data.link`, `files[0].url`, `$.data.link`
//! - `{regex:pattern}` or `{regex:pattern|group}` (group number or name)
//! - `{filename}`, `{input}`
//! - `{random:a|b|c}`, `{select:a|b}` (first option), `{prompt:title|default}` (default)
//! - `{base64:text}`
//!
//! Legacy syntax (ShareX 12 and earlier) is handled by [`expand_legacy`]: `$json:path$`,
//! `$regex:index,group$` (against the uploader's RegexList), `$response$`, `$header:Name$`,
//! `$filename$`, `$input$`, `$random:a|b$`.

use base64::Engine;
use serde_json::Value;

/// Everything a template can refer to. Response fields are empty before the request is sent.
#[derive(Clone, Debug, Default)]
pub struct Context {
    pub filename: String,
    pub input: String,
    pub response: String,
    pub response_url: String,
    pub headers: Vec<(String, String)>,
    /// Legacy `RegexList`.
    pub regex_list: Vec<String>,
    /// JSON-escape every value a function produces (for `Body: JSON` data templates).
    pub json_escape: bool,
}

impl Context {
    fn header(&self, name: &str) -> String {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone()).unwrap_or_default()
    }
}

pub fn expand(template: &str, ctx: &Context) -> String {
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() && matches!(chars[i + 1], '{' | '}' | '|' | '\\') {
            out.push(chars[i + 1]);
            i += 2;
        } else if c == '{' {
            match matching_brace(&chars, i) {
                Some(end) => {
                    let inner: String = chars[i + 1..end].iter().collect();
                    match evaluate(&inner, ctx) {
                        Some(value) if ctx.json_escape => out.push_str(&json_escape(&value)),
                        Some(value) => out.push_str(&value),
                        // Not a function (a JSON object in a request body, say): keep the braces
                        // and expand what's inside.
                        None => {
                            out.push('{');
                            out.push_str(&expand(&inner, ctx));
                            out.push('}');
                        }
                    }
                    i = end + 1;
                }
                None => {
                    out.push(c);
                    i += 1;
                }
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// Index of the `}` closing the `{` at `open`, honouring nesting and escapes.
fn matching_brace(chars: &[char], open: usize) -> Option<usize> {
    let mut depth = 0;
    let mut i = open;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 1,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Splits `s` at top-level occurrences of `sep` (not inside braces, not escaped). Escapes are kept
/// so the parts can be expanded afterwards.
fn split_top(s: &str, sep: char, max_parts: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut depth = 0;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            cur.push(c);
            if let Some(n) = chars.next() {
                cur.push(n);
            }
            continue;
        }
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
        if c == sep && depth == 0 && parts.len() + 1 < max_parts {
            parts.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    parts.push(cur);
    parts
}

fn evaluate(inner: &str, ctx: &Context) -> Option<String> {
    let mut name_args = split_top(inner, ':', 2);
    let name = name_args.remove(0).trim().to_ascii_lowercase();
    let raw_args = name_args.pop();
    // Function results are inserted raw; escaping applies once, at the outermost level.
    let plain = Context { json_escape: false, ..ctx.clone() };
    let args = |max: usize| -> Vec<String> {
        match &raw_args {
            Some(a) => split_top(a, '|', max).iter().map(|p| expand(p, &plain)).collect(),
            None => Vec::new(),
        }
    };
    Some(match name.as_str() {
        "response" => ctx.response.clone(),
        "responseurl" => ctx.response_url.clone(),
        "filename" => ctx.filename.clone(),
        "input" => ctx.input.clone(),
        "header" => args(1).first().map(|h| ctx.header(h.trim())).unwrap_or_default(),
        "json" => {
            let a = args(2);
            match a.as_slice() {
                [path] => json_lookup(&ctx.response, path),
                [source, path] => json_lookup(source, path),
                _ => String::new(),
            }
        }
        "regex" => {
            // The pattern itself is not template-expanded beyond escapes, so `\d` and friends survive.
            let parts = split_top(raw_args.as_deref().unwrap_or_default(), '|', 2);
            let pattern = unescape(&parts[0]);
            let group = parts.get(1).map(|g| expand(g, &plain));
            regex_lookup(&ctx.response, &pattern, group.as_deref())
        }
        "random" => {
            let a = args(usize::MAX);
            if a.is_empty() { String::new() } else { a[fastrand::usize(..a.len())].clone() }
        }
        "select" => args(usize::MAX).into_iter().next().unwrap_or_default(),
        "prompt" => args(2).get(1).cloned().unwrap_or_default(),
        "base64" => base64::engine::general_purpose::STANDARD.encode(args(1).first().cloned().unwrap_or_default()),
        _ => return None,
    })
}

/// Undoes `\{ \} \| \\` only, leaving other backslashes for the regex engine.
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(&n) = chars.peek()
            && matches!(n, '{' | '}' | '|' | '\\')
        {
            out.push(n);
            chars.next();
            continue;
        }
        out.push(c);
    }
    out
}

pub fn json_escape(s: &str) -> String {
    let quoted = serde_json::to_string(s).unwrap_or_default();
    quoted[1..quoted.len() - 1].to_string()
}

/// Looks up `path` in the JSON text `source`. Strings come back without quotes; objects and arrays
/// as JSON; anything missing as an empty string.
pub fn json_lookup(source: &str, path: &str) -> String {
    let Ok(root) = serde_json::from_str::<Value>(source.trim()) else { return String::new() };
    let mut cur = &root;
    for seg in path_segments(path) {
        let next = match seg {
            Seg::Key(k) => cur.get(&k),
            Seg::Index(i) => cur.get(i),
        };
        match next {
            Some(v) => cur = v,
            None => return String::new(),
        }
    }
    match cur {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

enum Seg {
    Key(String),
    Index(usize),
}

fn path_segments(path: &str) -> Vec<Seg> {
    let path = path.trim();
    let path = path.strip_prefix('$').unwrap_or(path);
    let mut segs = Vec::new();
    let mut key = String::new();
    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '.' => {
                if !key.is_empty() {
                    segs.push(Seg::Key(std::mem::take(&mut key)));
                }
            }
            '[' => {
                if !key.is_empty() {
                    segs.push(Seg::Key(std::mem::take(&mut key)));
                }
                let mut inside = String::new();
                for c in chars.by_ref() {
                    if c == ']' {
                        break;
                    }
                    inside.push(c);
                }
                let inside = inside.trim();
                if let Ok(i) = inside.parse::<usize>() {
                    segs.push(Seg::Index(i));
                } else {
                    segs.push(Seg::Key(inside.trim_matches(['\'', '"']).to_string()));
                }
            }
            _ => key.push(c),
        }
    }
    if !key.is_empty() {
        segs.push(Seg::Key(key));
    }
    segs
}

/// The first match of `pattern` in `text`: the whole match, or the numbered or named group.
pub fn regex_lookup(text: &str, pattern: &str, group: Option<&str>) -> String {
    let Ok(re) = regex::Regex::new(pattern) else { return String::new() };
    let Some(caps) = re.captures(text) else { return String::new() };
    let m = match group.map(str::trim) {
        None | Some("") => caps.get(0),
        Some(g) => match g.parse::<usize>() {
            Ok(n) => caps.get(n),
            Err(_) => caps.name(g),
        },
    };
    m.map(|m| m.as_str().to_string()).unwrap_or_default()
}

/// The pre-13 `$...$` syntax.
pub fn expand_legacy(template: &str, ctx: &Context) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('$') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('$') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let inner = &after[..end];
        match legacy_value(inner, ctx) {
            Some(v) => out.push_str(&v),
            None => {
                // Not a function: keep the `$` and carry on after it.
                out.push('$');
                rest = after;
                continue;
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn legacy_value(inner: &str, ctx: &Context) -> Option<String> {
    let (name, arg) = match inner.split_once(':') {
        Some((n, a)) => (n.to_ascii_lowercase(), Some(a)),
        None => (inner.to_ascii_lowercase(), None),
    };
    Some(match (name.as_str(), arg) {
        ("response", None) => ctx.response.clone(),
        ("responseurl", None) => ctx.response_url.clone(),
        ("filename", None) => ctx.filename.clone(),
        ("input", None) => ctx.input.clone(),
        ("header", Some(h)) => ctx.header(h.trim()),
        ("json", Some(p)) => json_lookup(&ctx.response, p),
        ("random", Some(a)) => {
            let opts: Vec<&str> = a.split('|').collect();
            opts[fastrand::usize(..opts.len())].to_string()
        }
        ("regex", Some(a)) => {
            let (index, group) = a.split_once(',').unwrap_or((a, ""));
            let index: usize = index.trim().parse().ok()?;
            let pattern = ctx.regex_list.get(index.checked_sub(1)?)?;
            regex_lookup(&ctx.response, pattern, Some(group))
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Context {
        Context {
            filename: "shot.png".into(),
            response: r#"{"status":200,"data":{"link":"https://i.example.com/a.png","id":"a1"},"files":[{"url":"https://f.example.com/x"}],"ok":true}"#.into(),
            response_url: "https://example.com/upload".into(),
            headers: vec![("Location".into(), "https://example.com/l".into())],
            ..Default::default()
        }
    }

    #[test]
    fn plain_text_passes_through() {
        assert_eq!(expand("https://example.com/a?b=c", &ctx()), "https://example.com/a?b=c");
    }

    #[test]
    fn json_paths() {
        let c = ctx();
        assert_eq!(expand("{json:data.link}", &c), "https://i.example.com/a.png");
        assert_eq!(expand("{json:$.data.id}", &c), "a1");
        assert_eq!(expand("{json:files[0].url}", &c), "https://f.example.com/x");
        assert_eq!(expand("{json:status}", &c), "200");
        assert_eq!(expand("{json:ok}", &c), "true");
        assert_eq!(expand("{json:missing.key}", &c), "");
        assert_eq!(expand("{json:['data']['id']}", &c), "a1");
        assert_eq!(expand(r#"{json:{"k":"v"}|k}"#, &c), "v");
    }

    #[test]
    fn nested_and_combined() {
        let c = ctx();
        assert_eq!(
            expand("https://del.example.com/{json:data.id}?f={filename}", &c),
            "https://del.example.com/a1?f=shot.png"
        );
        assert_eq!(expand("{base64:{filename}}", &c), "c2hvdC5wbmc=");
        assert_eq!(expand("{header:location}", &c), "https://example.com/l");
        assert_eq!(expand("{responseurl}", &c), "https://example.com/upload");
    }

    #[test]
    fn regex_groups() {
        let c = Context { response: "<a href=\"https://x.example/abc\">link</a> id=42".into(), ..Default::default() };
        assert_eq!(expand(r#"{regex:href="([^"]+)"|1}"#, &c), "https://x.example/abc");
        assert_eq!(expand(r"{regex:id=(?P<n>\d+)|n}", &c), "42");
        assert_eq!(expand(r"{regex:id=\d+}", &c), "id=42");
        assert_eq!(expand(r"{regex:(q\|x)\.example}", &c), "x.example");
    }

    #[test]
    fn escapes_and_unknowns() {
        let c = ctx();
        assert_eq!(expand(r"\{literal\}", &c), "{literal}");
        assert_eq!(expand("{nope:1}", &c), "{nope:1}");
        assert_eq!(expand("unclosed {json:data", &c), "unclosed {json:data");
        assert_eq!(expand("{select:a|b}", &c), "a");
        assert_eq!(expand("{prompt:Title|fallback}", &c), "fallback");
        let r = expand("{random:x|y}", &c);
        assert!(r == "x" || r == "y");
    }

    #[test]
    fn json_escaping_for_bodies() {
        let c = Context { input: "say \"hi\"\n".into(), json_escape: true, ..Default::default() };
        assert_eq!(expand(r#"{"text":"{input}"}"#, &c), r#"{"text":"say \"hi\"\n"}"#);
    }

    #[test]
    fn legacy_syntax() {
        let mut c = ctx();
        c.regex_list = vec![r#""id":"(\w+)""#.into()];
        assert_eq!(expand_legacy("$json:data.link$", &c), "https://i.example.com/a.png");
        assert_eq!(expand_legacy("https://x/$regex:1,1$", &c), "https://x/a1");
        assert_eq!(expand_legacy("$header:Location$", &c), "https://example.com/l");
        assert_eq!(expand_legacy("costs $5 and $filename$", &c), "costs $5 and shot.png");
    }
}
