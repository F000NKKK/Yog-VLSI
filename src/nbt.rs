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
/// double-quoted string (the loader may print it with or without a space
/// after the colon, or with the key quoted).
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
        let Some(value) = value.trim_start().strip_prefix('"') else { continue };
        return unescape_until_quote(value);
    }
}

fn unescape_until_quote(s: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push(chars.next()?),
            '"' => return Some(out),
            c => out.push(c),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(read(r#"{Tag:"unterminated}"#, "Tag"), None);
    }
}
