use crate::model::KeyValue;
use std::collections::HashMap;

fn valid_name(n: &str) -> bool {
    !n.is_empty() && n.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

pub fn to_map(vars: &[KeyValue]) -> HashMap<String, String> {
    vars.iter()
        .filter(|v| v.enabled && !v.key.trim().is_empty())
        .map(|v| (v.key.trim().to_string(), v.value.clone()))
        .collect()
}

/// Replaces `{{name}}` with its value. Unknown names are left untouched and
/// reported in the returned list (deduplicated, in order of appearance).
pub fn interpolate(text: &str, vars: &HashMap<String, String>) -> (String, Vec<String>) {
    let mut out = String::with_capacity(text.len());
    let mut missing: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let name = after[..end].trim();
                if !valid_name(name) {
                    // Not a variable (e.g. `{{}}`, `{{ a b }}`): keep the braces literal.
                    out.push_str("{{");
                    rest = after;
                    continue;
                }
                match vars.get(name) {
                    Some(v) => out.push_str(v),
                    None => {
                        out.push_str(&rest[start..start + 2 + end + 2]);
                        if !missing.iter().any(|m| m == name) {
                            missing.push(name.to_string());
                        }
                    }
                }
                rest = &after[end + 2..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    (out, missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn replaces_known_vars() {
        let (s, m) = interpolate("{{host}}/users/{{ id }}", &map(&[("host", "http://a"), ("id", "7")]));
        assert_eq!(s, "http://a/users/7");
        assert!(m.is_empty());
    }

    #[test]
    fn keeps_and_reports_unknown() {
        let (s, m) = interpolate("{{a}} {{b}} {{a}}", &map(&[("b", "x")]));
        assert_eq!(s, "{{a}} x {{a}}");
        assert_eq!(m, vec!["a"]);
    }

    #[test]
    fn unterminated_braces_are_literal() {
        let (s, m) = interpolate("x {{oops", &map(&[]));
        assert_eq!(s, "x {{oops");
        assert!(m.is_empty());
    }

    #[test]
    fn values_are_not_reinterpolated() {
        let (s, _) = interpolate("{{a}}", &map(&[("a", "{{b}}"), ("b", "no")]));
        assert_eq!(s, "{{b}}");
    }

    #[test]
    fn non_variable_braces_stay_literal() {
        for text in ["{{}}", "{{ a b }}", "json: {{\"k\": 1}}"] {
            let (s, m) = interpolate(text, &map(&[("a", "1")]));
            assert_eq!(s, text);
            assert!(m.is_empty(), "{text}");
        }
        let (s, m) = interpolate("{{{{a}}", &map(&[("a", "1")]));
        assert_eq!(s, "{{1");
        assert!(m.is_empty());
    }

    #[test]
    fn handles_unicode() {
        let (s, _) = interpolate("é{{a}}ü", &map(&[("a", "→")]));
        assert_eq!(s, "é→ü");
    }

    #[test]
    fn disabled_vars_are_ignored() {
        let mut v = KeyValue::new("a", "1");
        v.enabled = false;
        assert!(to_map(&[v]).is_empty());
    }
}
