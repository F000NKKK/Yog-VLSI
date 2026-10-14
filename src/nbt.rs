//! JSON payloads stored in item / block-entity NBT as one SNBT string tag.
//!
//! The loader hands NBT around as SNBT text; this mod keeps its structured
//! data (chip metadata, blueprints, port labels) as a JSON string under one
//! tag, so only that tag needs (un)escaping — no general SNBT parser.

/// `{<tag>:"<escaped value>"}`.
pub fn wrap(tag: &str, value: &str) -> String {
    let mut out = String::with_capacity(value.len() + tag.len() + 6);
    out.push('{');
    out.push_str(tag);
    out.push_str(":\"");
    for c in value.chars() {
        if matches!(c, '\\' | '"') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push_str("\"}");
    out
}

/// The unescaped string value of `tag` in `snbt`, if it's there as a
/// quoted string — double or single quotes: Minecraft prints a string that
/// holds `"` in single quotes. The key may be quoted, the colon followed by
/// a space.
pub fn read(snbt: &str, tag: &str) -> Option<String> {
    let mut rest = snbt;
    loop {
        let at = rest.find(tag)?;
        let before = rest[..at].chars().next_back();
        let after = rest[at + tag.len()..].trim_start_matches('"').trim_start();
        rest = &rest[at + tag.len()..];
        // A key starts after `{`, `,`, whitespace or its opening quote.
        if !matches!(before, None | Some('{' | ',' | ' ' | '"')) {
            continue;
        }
        let Some(value) = after.strip_prefix(':') else { continue };
        let value = value.trim_start();
        let Some(quote) = value.chars().next().filter(|c| matches!(c, '"' | '\'')) else { continue };
        return unescape_until(&value[1..], quote);
    }
}

fn unescape_until(s: &str, quote: char) -> Option<String> {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push(chars.next()?),
            c if c == quote => return Some(out),
            c => out.push(c),
        }
    }
    None
}

/// Where the value of top-level key `key` of compound `snbt` is (any
/// value: a list, a compound, a number…), skipping nested ones and strings.
pub fn top_value(snbt: &str, key: &str) -> Option<std::ops::Range<usize>> {
    let b = snbt.as_bytes();
    let (mut depth, mut i) = (0usize, 0usize);
    let mut key_start: Option<usize> = None;
    let mut value: Option<usize> = None;
    while i < b.len() {
        let c = b[i];
        match c {
            b'"' | b'\'' => {
                // A string: to its closing quote.
                i += 1;
                while i < b.len() && b[i] != c {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'{' | b'[' => {
                depth += 1;
                if depth == 1 {
                    key_start = Some(i + 1);
                }
            }
            b'}' | b']' => {
                if depth == 1
                    && let Some(v) = value
                {
                    return Some(v..i);
                }
                depth = depth.saturating_sub(1);
            }
            b',' if depth == 1 => {
                if let Some(v) = value {
                    return Some(v..i);
                }
                key_start = Some(i + 1);
            }
            b':' if depth == 1 && value.is_none() => {
                if let Some(k) = key_start.take()
                    && snbt[k..i].trim().trim_matches('"') == key
                {
                    value = Some(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// `snbt` with top-level key `key` set to `value` (added if it isn't there).
pub fn with_top_value(snbt: &str, key: &str, value: &str) -> String {
    match top_value(snbt, key) {
        Some(r) => format!("{}{value}{}", &snbt[..r.start], &snbt[r.end..]),
        None => match snbt.trim_end().strip_suffix('}') {
            Some(body) if body.trim() == "{" => format!("{{{key}:{value}}}"),
            Some(body) => format!("{body},{key}:{value}}}"),
            None => format!("{{{key}:{value}}}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_and_replaces_a_top_level_value() {
        let snbt = r#"{Air:300s,Inventory:[{Slot:0b,components:{"minecraft:custom_name":'"a,b]"'},id:"minecraft:stone"}],Pos:[1.0d,2.0d,3.0d]}"#;
        let r = top_value(snbt, "Inventory").unwrap();
        assert!(snbt[r.clone()].starts_with("[{Slot:0b") && snbt[r].ends_with("}]"));
        assert_eq!(&snbt[top_value(snbt, "Air").unwrap()], "300s");
        assert_eq!(top_value(snbt, "Slot"), None, "not a top-level key");
        let out = with_top_value(snbt, "Inventory", "[]");
        assert_eq!(out, "{Air:300s,Inventory:[],Pos:[1.0d,2.0d,3.0d]}");
        assert_eq!(with_top_value("{}", "Inventory", "[]"), "{Inventory:[]}");
        assert_eq!(with_top_value("{A:1}", "Inventory", "[]"), "{A:1,Inventory:[]}");
    }

    #[test]
    fn round_trips_json() {
        let json = r#"{"name":"a \"q\" \\ b","n":1}"#;
        let snbt = wrap("Tag", json);
        assert_eq!(read(&snbt, "Tag").as_deref(), Some(json));
    }

    #[test]
    fn reads_loader_spellings() {
        assert_eq!(read(r#"{Other:1b,Tag: "x\"y"}"#, "Tag").as_deref(), Some("x\"y"));
        assert_eq!(read(r#"{"Tag":"v"}"#, "Tag").as_deref(), Some("v"));
        assert_eq!(read(r#"{NotTag:"a",Tag:"b"}"#, "Tag").as_deref(), Some("b"));
        assert_eq!(read("{}", "Tag"), None);
        // How Minecraft prints a string holding double quotes.
        assert_eq!(read(r#"{Tag:'{"name":"it\'s"}'}"#, "Tag").as_deref(), Some(r#"{"name":"it's"}"#));
        assert_eq!(read(r#"{Tag:"unterminated}"#, "Tag"), None);
    }
}
