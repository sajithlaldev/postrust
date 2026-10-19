//! "Copy as cURL" followed by pasting that command back must describe the same request.

use postrust::curl;
use postrust::http::{Prepared, prepare, to_curl};
use postrust::model::*;
use std::collections::HashMap;

fn vars() -> HashMap<String, String> {
    HashMap::new()
}

/// What is actually sent, minus the bits that legitimately differ: the JSON Content-Type is added
/// by send() for the original but written explicitly into the command.
fn essence(p: &Prepared) -> (Method, String, Vec<(String, String)>, Option<String>, String) {
    let mut headers: Vec<(String, String)> = p
        .headers
        .iter()
        .filter(|(k, v)| !(k.eq_ignore_ascii_case("content-type") && v == "application/json"))
        .cloned()
        .collect();
    headers.sort();
    // Basic credentials are compared as sent ("user:pass"); curl's -u syntax splits at the first
    // colon, so a username containing ':' is re-split but produces the same Authorization header.
    let basic = p.basic.as_ref().map(|(u, pw)| format!("{u}:{pw}"));
    (p.method, p.url.clone(), headers, basic, format!("{:?}", p.body))
}

fn round_trip(original: &Request) {
    let sent = prepare(original, &vars()).expect("original prepares");
    let command = to_curl(&sent);
    assert!(curl::looks_like_curl(&command), "{command}");
    let imported = curl::parse(&command).unwrap_or_else(|e| panic!("could not parse our own output ({e}):\n{command}"));
    assert!(
        imported.warnings.is_empty(),
        "unexpected warnings {:?} for:\n{command}",
        imported.warnings
    );
    let again = prepare(&imported.request, &vars()).expect("imported prepares");
    assert_eq!(
        essence(&sent),
        essence(&again),
        "request changed through cURL:\n{command}"
    );
}

fn base(method: Method, url: &str) -> Request {
    Request {
        method,
        url: url.into(),
        ..Default::default()
    }
}

#[test]
fn simple_requests_survive_the_round_trip() {
    round_trip(&base(Method::Get, "https://api.test/users"));
    round_trip(&base(Method::Delete, "https://api.test/users/1"));
    round_trip(&base(Method::Head, "https://api.test/ping"));
    round_trip(&base(Method::Options, "https://api.test/"));
    round_trip(&base(Method::Post, "https://api.test/empty-post"));
    let mut r = base(Method::Get, "https://api.test/s");
    r.params = vec![KeyValue::new("q", "a b&c"), KeyValue::new("page", "2")];
    round_trip(&r);
}

#[test]
fn bodies_survive_the_round_trip() {
    let mut json = base(Method::Post, "https://api.test/users");
    json.body_kind = BodyKind::Raw;
    json.body = "{\n  \"name\": \"it's \u{e9}\\\"quoted\\\"\",\n  \"n\": [1, 2]\n}".into();
    round_trip(&json);

    for method in [Method::Put, Method::Patch] {
        let mut r = base(method, "https://api.test/x");
        r.body_kind = BodyKind::Raw;
        r.body = r#"{"a":1}"#.into();
        round_trip(&r);
    }

    let mut text = base(Method::Post, "https://api.test/note");
    text.body_kind = BodyKind::Raw;
    text.body = "plain text, with 'quotes' and\nnewlines".into();
    round_trip(&text);

    let mut looks_like_form = base(Method::Post, "https://api.test/note");
    looks_like_form.body_kind = BodyKind::Raw;
    looks_like_form.body = "a=b&c=d".into(); // raw text that merely resembles a form
    round_trip(&looks_like_form);

    let mut form = base(Method::Post, "https://api.test/login");
    form.body_kind = BodyKind::Form;
    form.form = vec![
        KeyValue::new("user name", "ada l"),
        KeyValue::new("p&ss=word", "x+y%z"),
        KeyValue::new("city", "Z\u{fc}rich"),
    ];
    round_trip(&form);

    let mut get_with_body = base(Method::Get, "https://api.test/search");
    get_with_body.body_kind = BodyKind::Raw;
    get_with_body.body = r#"{"q":"x"}"#.into();
    round_trip(&get_with_body);
}

#[test]
fn headers_and_auth_survive_the_round_trip() {
    let mut r = base(Method::Post, "https://api.test/x");
    r.headers = vec![
        KeyValue::new("X-Id", "7"),
        KeyValue::new("X-Empty", ""),
        KeyValue::new("Accept", "text/plain"),
    ];
    round_trip(&r);

    r.auth = Auth::Bearer {
        token: "abc.def-ghi".into(),
    };
    round_trip(&r);

    r.auth = Auth::Basic {
        username: "al:ice".into(),
        password: "p w'd".into(),
    };
    round_trip(&r);

    r.auth = Auth::ApiKey {
        name: "X-Api-Key".into(),
        value: "k e y".into(),
    };
    round_trip(&r);

    let mut ct = base(Method::Post, "https://api.test/x");
    ct.headers = vec![KeyValue::new("Content-Type", "application/xml")];
    ct.body_kind = BodyKind::Raw;
    ct.body = "<a b='1'/>".into();
    round_trip(&ct);
}

#[test]
fn awkward_urls_survive_the_round_trip() {
    for url in [
        "https://api.test/items?filter[name]=x&ids[]=1&ids[]=2",
        "https://api.test/a%20b?x=%7B1%7D",
        "http://localhost:3000/it's",
        "https://api.test/\u{e9}\u{20ac}",
        "https://user@api.test:8443/p?x=1#frag",
    ] {
        round_trip(&base(Method::Get, url));
    }
}

#[test]
fn variables_are_resolved_before_export_so_the_import_has_real_values() {
    let mut r = base(Method::Post, "{{base}}/users/{{id}}");
    r.headers = vec![KeyValue::new("X-Token", "{{token}}")];
    r.body_kind = BodyKind::Raw;
    r.body = r#"{"id":{{id}}}"#.into();
    let vars: HashMap<String, String> = [("base", "https://api.test"), ("id", "42"), ("token", "t0k")]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let command = to_curl(&prepare(&r, &vars).unwrap());
    let imported = curl::parse(&command).unwrap().request;
    assert_eq!(imported.url, "https://api.test/users/42");
    assert_eq!(imported.body, r#"{"id":42}"#);
    assert!(imported.headers.iter().any(|h| h.key == "X-Token" && h.value == "t0k"));
}
