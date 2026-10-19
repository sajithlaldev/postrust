//! JSON helpers that don't depend on the UI.

/// If `text[i..]` starts a `{{variable}}` placeholder, returns the index just past its closing
/// `}}`. The name may not contain a newline, quote or brace, so the scan is bounded by the
/// first such character instead of walking the whole remaining text.
pub fn find_placeholder(text: &str, i: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(i) != Some(&b'{') || bytes.get(i + 1) != Some(&b'{') {
        return None;
    }
    let mut j = i + 2;
    while j < bytes.len() {
        match bytes[j] {
            b'}' if bytes.get(j + 1) == Some(&b'}') => {
                // Same rule as `vars::interpolate`, so the editor only treats real variables as such.
                return crate::vars::valid_name(text[i + 2..j].trim()).then_some(j + 2);
            }
            b'\n' | b'"' | b'{' => return None,
            _ => j += 1,
        }
    }
    None
}

/// Pretty-prints `text` as JSON. Unquoted `{{variable}}` placeholders (e.g. `{"id": {{id}}}`)
/// are preserved, which plain JSON parsing would reject. Key order and number precision
/// are kept as written. Duplicate object keys collapse to the last value.
pub fn beautify(text: &str) -> Result<String, String> {
    if text.trim().is_empty() {
        return Err("the body is empty".into());
    }
    // A marker that does not occur in the input, so user data can never be mistaken for a token.
    let mut prefix = String::from("__postrust_var_");
    while text.contains(&prefix) {
        prefix.push('x');
    }
    let (masked, vars) = mask_placeholders(text, &prefix);
    let value: serde_json::Value = serde_json::from_str(&masked).map_err(|e| {
        if vars.is_empty() {
            e.to_string()
        } else {
            // Positions refer to the text with placeholders swapped for tokens.
            format!("{e}; the position is approximate because the body contains {{{{variables}}}}")
        }
    })?;
    let pretty = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    Ok(restore_placeholders(&pretty, &prefix, &vars))
}

/// Single pass: swaps each quoted token back for the original `{{variable}}` text.
fn restore_placeholders(pretty: &str, prefix: &str, vars: &[String]) -> String {
    let marker = format!("\"{prefix}");
    let mut out = String::with_capacity(pretty.len());
    let mut rest = pretty;
    while let Some(pos) = rest.find(&marker) {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + marker.len()..];
        let index = after
            .find("__\"")
            .and_then(|end| Some((end, after[..end].parse::<usize>().ok()?)));
        match index {
            Some((end, n)) if n < vars.len() => {
                out.push_str(&vars[n]);
                rest = &after[end + 3..];
            }
            _ => {
                out.push_str(&marker);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Replaces `{{name}}` outside of string literals with a quoted token so the text parses.
fn mask_placeholders(text: &str, prefix: &str) -> (String, Vec<String>) {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut vars = Vec::new();
    let mut i = 0;
    let mut in_string = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            // Copy the whole char so multi-byte UTF-8 stays intact.
            let ch_len = utf8_len(c);
            out.push_str(&text[i..i + ch_len]);
            if c == b'\\' && i + 1 < bytes.len() {
                let next = utf8_len(bytes[i + 1]);
                out.push_str(&text[i + 1..i + 1 + next]);
                i += 1 + next;
                continue;
            }
            if c == b'"' {
                in_string = false;
            }
            i += ch_len;
        } else if c == b'"' {
            in_string = true;
            out.push('"');
            i += 1;
        } else if let Some(end) = find_placeholder(text, i) {
            out.push('"');
            out.push_str(prefix);
            out.push_str(&vars.len().to_string());
            out.push_str("__\"");
            vars.push(text[i..end].to_string());
            i = end;
        } else {
            let ch_len = utf8_len(c);
            out.push_str(&text[i..i + ch_len]);
            i += ch_len;
        }
    }
    (out, vars)
}

fn utf8_len(first: u8) -> usize {
    match first {
        0..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_prints_plain_json() {
        assert_eq!(
            beautify(r#"{"a":1,"b":[1,2]}"#).unwrap(),
            "{\n  \"a\": 1,\n  \"b\": [\n    1,\n    2\n  ]\n}"
        );
    }

    #[test]
    fn keeps_key_order_and_big_numbers() {
        let out = beautify(r#"{"z":1,"a":12345678901234567890123,"m":1.50}"#).unwrap();
        let z = out.find("\"z\"").unwrap();
        let a = out.find("\"a\"").unwrap();
        assert!(z < a);
        assert!(out.contains("12345678901234567890123"));
    }

    #[test]
    fn preserves_unquoted_placeholders() {
        let out = beautify(r#"{"id":{{id}},"name":"{{name}}","list":[{{a}},{{ b }}]}"#).unwrap();
        assert!(out.contains("\"id\": {{id}}"), "{out}");
        assert!(out.contains("\"name\": \"{{name}}\""), "{out}");
        assert!(out.contains("{{a}}") && out.contains("{{ b }}"), "{out}");
        assert!(!out.contains("__postrust_var"), "{out}");
    }

    #[test]
    fn braces_inside_strings_are_untouched() {
        let out = beautify(r#"{"t":"{{x}} and \" {{y}}"}"#).unwrap();
        assert!(out.contains(r#""{{x}} and \" {{y}}""#), "{out}");
    }

    #[test]
    fn handles_unicode() {
        assert!(beautify(r#"{"k":"é€😀","v":{{x}}}"#).unwrap().contains("é€😀"));
    }

    #[test]
    fn placeholder_rule() {
        assert_eq!(find_placeholder("{{a}} x", 0), Some(5));
        assert_eq!(find_placeholder("{{ a }}", 0), Some(7), "spaces around a name are fine");
        assert_eq!(find_placeholder("{{ a b }}", 0), None, "not a valid variable name");
        assert_eq!(find_placeholder("{{}}", 0), None);
        assert_eq!(find_placeholder("{{ {{x}} }}", 0), None, "nested brace");
        assert_eq!(find_placeholder("{{a\n}}", 0), None);
        assert_eq!(find_placeholder("{{a\"}}", 0), None);
        assert_eq!(find_placeholder("{{unterminated", 0), None);
        assert_eq!(find_placeholder("{x}}", 0), None);
        assert_eq!(find_placeholder("a{{b}}", 1), Some(6));
    }

    #[test]
    fn many_unterminated_openers_are_linear() {
        let text = "{{".repeat(200_000);
        let start = std::time::Instant::now();
        assert!(beautify(&text).is_err());
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn notes_approximate_position_when_variables_present() {
        let with = beautify(r#"{"a": {{x}}, "b": }"#).unwrap_err();
        assert!(with.contains("approximate"), "{with}");
        let without = beautify(r#"{"a": }"#).unwrap_err();
        assert!(!without.contains("approximate"), "{without}");
    }

    #[test]
    fn user_text_that_looks_like_a_token_is_not_replaced() {
        let out = beautify(r#"{"s":"__postrust_var_0__","v":{{x}}}"#).unwrap();
        assert!(out.contains("\"__postrust_var_0__\""), "{out}");
        assert!(out.contains("\"v\": {{x}}"), "{out}");
    }

    #[test]
    fn many_variables_are_handled_in_one_pass() {
        let body = format!("[{}0]", "{{a}},".repeat(50_000));
        let start = std::time::Instant::now();
        let out = beautify(&body).unwrap();
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
        assert_eq!(out.matches("{{a}}").count(), 50_000);
    }

    #[test]
    fn invalid_variable_names_are_not_placeholders() {
        assert!(beautify(r#"{"a": {{}}}"#).is_err());
        assert!(beautify(r#"{"a": {{x y}}}"#).is_err());
    }

    #[test]
    fn reports_errors() {
        assert!(beautify("").is_err());
        assert!(beautify("   ").is_err());
        assert!(beautify("{oops").is_err());
        assert!(beautify(r#"{"a": }"#).is_err());
    }
}
