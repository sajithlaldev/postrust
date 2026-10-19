use postrust::http::{self, prepare};
use postrust::model::*;
use std::collections::HashMap;
use std::time::Duration;
use wiremock::matchers::{body_string, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn no_vars() -> HashMap<String, String> {
    HashMap::new()
}

async fn run(req: &Request, vars: &HashMap<String, String>) -> anyhow::Result<http::ResponseData> {
    http::send(prepare(req, vars)?, Duration::from_secs(5)).await
}

#[tokio::test]
async fn get_with_query_and_bearer() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/items"))
        .and(query_param("q", "rust"))
        .and(header("authorization", "Bearer tok"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
        .mount(&server)
        .await;

    let mut req = Request::new("t");
    req.url = format!("{}/items", server.uri());
    req.params = vec![KeyValue::new("q", "rust")];
    req.auth = Auth::Bearer { token: "tok".into() };
    let resp = run(&req, &no_vars()).await.unwrap();
    assert_eq!(resp.status, 200);
    assert!(resp.pretty_text().contains("\"ok\": true"));
}

#[tokio::test]
async fn post_json_gets_content_type_and_variables() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/users"))
        .and(header("content-type", "application/json"))
        .and(body_string(r#"{"name":"ada"}"#))
        .respond_with(ResponseTemplate::new(201))
        .mount(&server)
        .await;

    let mut req = Request::new("t");
    req.method = Method::Post;
    req.url = "{{base}}/users".into();
    req.body_kind = BodyKind::Raw;
    req.body = r#"{"name":"{{who}}"}"#.into();
    let vars = HashMap::from([("base".into(), server.uri()), ("who".into(), "ada".into())]);
    assert_eq!(run(&req, &vars).await.unwrap().status, 201);
}

#[tokio::test]
async fn form_body_is_urlencoded() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(header("content-type", "application/x-www-form-urlencoded"))
        .and(body_string("a=1&b=x+y"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let mut req = Request::new("t");
    req.method = Method::Post;
    req.url = server.uri();
    req.body_kind = BodyKind::Form;
    req.form = vec![KeyValue::new("a", "1"), KeyValue::new("b", "x y")];
    assert_eq!(run(&req, &no_vars()).await.unwrap().status, 200);
}

#[tokio::test]
async fn basic_auth_and_api_key() {
    let server = MockServer::start().await;
    Mock::given(header("authorization", "Basic dTpw"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(header("x-key", "k"))
        .respond_with(ResponseTemplate::new(202))
        .mount(&server)
        .await;

    let mut req = Request::new("t");
    req.url = server.uri();
    req.auth = Auth::Basic {
        username: "u".into(),
        password: "p".into(),
    };
    assert_eq!(run(&req, &no_vars()).await.unwrap().status, 200);
    req.auth = Auth::ApiKey {
        name: "X-Key".into(),
        value: "k".into(),
    };
    assert_eq!(run(&req, &no_vars()).await.unwrap().status, 202);
}

#[tokio::test]
async fn error_status_is_a_response_not_an_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404).set_body_string("nope"))
        .mount(&server)
        .await;
    let mut req = Request::new("t");
    req.url = server.uri();
    let resp = run(&req, &no_vars()).await.unwrap();
    assert_eq!((resp.status, resp.text().as_str()), (404, "nope"));
}

#[tokio::test]
async fn follows_redirects() {
    let server = MockServer::start().await;
    Mock::given(path("/a"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/b"))
        .mount(&server)
        .await;
    Mock::given(path("/b"))
        .respond_with(ResponseTemplate::new(200).set_body_string("done"))
        .mount(&server)
        .await;
    let mut req = Request::new("t");
    req.url = format!("{}/a", server.uri());
    assert_eq!(run(&req, &no_vars()).await.unwrap().text(), "done");
}

#[tokio::test]
async fn binary_body_is_detected() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0xff, 0xfe, 0x00, 0x80]))
        .mount(&server)
        .await;
    let mut req = Request::new("t");
    req.url = server.uri();
    let resp = run(&req, &no_vars()).await.unwrap();
    assert!(!resp.is_text());
    assert_eq!(resp.size, 4);
}

#[tokio::test]
async fn timeout_is_reported() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(3)))
        .mount(&server)
        .await;
    let mut req = Request::new("t");
    req.url = server.uri();
    let err = http::send(prepare(&req, &no_vars()).unwrap(), Duration::from_millis(200))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("timed out"), "{err}");
}

#[tokio::test]
async fn connection_refused_is_an_error() {
    let mut req = Request::new("t");
    req.url = "http://127.0.0.1:1".into();
    assert!(run(&req, &no_vars()).await.is_err());
}

#[tokio::test]
async fn api_key_is_not_forwarded_across_hosts() {
    let target = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("leaked?"))
        .mount(&target)
        .await;
    let origin = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", format!("{}/x", target.uri())))
        .mount(&origin)
        .await;

    let mut req = Request::new("t");
    req.url = origin.uri();
    req.auth = Auth::ApiKey {
        name: "X-Key".into(),
        value: "secret".into(),
    };
    // Redirect is not followed, so the user sees the 302 itself and the target gets nothing.
    let resp = run(&req, &no_vars()).await.unwrap();
    assert_eq!(resp.status, 302);
    assert!(target.received_requests().await.unwrap().is_empty());

    // Without an API key the redirect is followed as usual.
    req.auth = Auth::None;
    assert_eq!(run(&req, &no_vars()).await.unwrap().text(), "leaked?");
}

#[tokio::test]
async fn oversized_body_is_truncated_at_a_char_boundary() {
    let server = MockServer::start().await;
    // 3-byte chars so the 20 MiB cut falls mid-character (20 MiB is not a multiple of 3).
    let body = "€".repeat(http::MAX_BODY_BYTES / 3 + 10);
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let mut req = Request::new("t");
    req.url = server.uri();
    let resp = run(&req, &no_vars()).await.unwrap();
    assert!(resp.truncated);
    assert!(resp.body.len() <= http::MAX_BODY_BYTES);
    assert!(resp.is_text(), "cut character should be trimmed, not shown as binary");
}

#[tokio::test]
async fn manual_authorization_header_is_replaced_by_auth_tab() {
    let server = MockServer::start().await;
    Mock::given(header("authorization", "Bearer real"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let mut req = Request::new("t");
    req.url = server.uri();
    req.headers = vec![KeyValue::new("Authorization", "stale")];
    req.auth = Auth::Bearer { token: "real".into() };
    assert_eq!(run(&req, &no_vars()).await.unwrap().status, 200);
}
