use crate::model::{Auth, BodyKind, Method, Request};
use crate::vars;
use anyhow::{Result, anyhow};
use futures_util::StreamExt as _;
use std::collections::HashMap;
use std::time::Duration;
use web_time::Instant;

/// Responses larger than this are truncated (the rest is dropped while reading).
pub const MAX_BODY_BYTES: usize = 20 * 1024 * 1024;
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone, PartialEq)]
pub enum PreparedBody {
    None,
    Raw(String),
    Form(Vec<(String, String)>),
}

/// A request with variables resolved and params merged into the URL.
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub basic: Option<(String, String)>,
    pub body: PreparedBody,
    pub missing_vars: Vec<String>,
    /// Name of the API-key header, if any; it must not follow a cross-host redirect.
    pub secret_header: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResponseData {
    pub status: u16,
    pub reason: String,
    pub elapsed_ms: u128,
    pub size: usize,
    pub truncated: bool,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl ResponseData {
    /// UTF-8, or a textual Content-Type (which may use another charset).
    pub fn is_text(&self) -> bool {
        std::str::from_utf8(&self.body).is_ok() || self.has_text_content_type()
    }

    fn has_text_content_type(&self) -> bool {
        let ct = self.content_type().unwrap_or("").to_ascii_lowercase();
        ct.starts_with("text/") || ct.contains("json") || ct.contains("xml") || ct.contains("javascript")
    }

    /// Body decoded with the charset from Content-Type (UTF-8 when absent or unknown).
    pub fn text(&self) -> String {
        if let Ok(s) = std::str::from_utf8(&self.body) {
            return s.to_string();
        }
        let label = self
            .content_type()
            .and_then(|ct| ct.to_ascii_lowercase().split("charset=").nth(1).map(str::to_string))
            .map(|l| l.split(';').next().unwrap_or("").trim().trim_matches('"').to_string())
            .unwrap_or_default();
        let enc = encoding_rs::Encoding::for_label(label.as_bytes()).unwrap_or(encoding_rs::UTF_8);
        enc.decode(&self.body).0.into_owned()
    }

    pub fn content_type(&self) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.as_str())
    }
}

/// Pretty-printed `text` if it parses as JSON (key order and number precision preserved).
pub fn pretty_json(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    // A bare `42` or `true` isn't worth a JSON view (and is common in plain-text bodies).
    if !(value.is_object() || value.is_array()) {
        return None;
    }
    serde_json::to_string_pretty(&value).ok()
}

/// True when the text *starts with* `scheme://` (a `://` inside the query doesn't count).
fn has_scheme(url: &str) -> bool {
    match url.find("://") {
        Some(i) if i > 0 => url[..i]
            .chars()
            .enumerate()
            .all(|(n, c)| c.is_ascii_alphabetic() || (n > 0 && (c.is_ascii_digit() || matches!(c, '+' | '-' | '.')))),
        _ => false,
    }
}

/// Host part of a scheme-less URL: no userinfo, no port; IPv6 keeps its brackets.
fn host_of(url: &str) -> &str {
    let authority = url.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if authority.starts_with('[') {
        match authority.find(']') {
            Some(i) => &authority[..=i],
            None => authority,
        }
    } else {
        authority.split(':').next().unwrap_or("")
    }
}

/// Hosts that almost certainly speak plain http: loopback, private/LAN and link-local IPv4,
/// `0.0.0.0`, IPv6 loopback, `localhost` and mDNS `.local` names.
fn is_local_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    if host == "localhost" || host == "[::1]" || host.ends_with(".local") || host.ends_with(".localhost") {
        return true;
    }
    host.parse::<std::net::Ipv4Addr>()
        .is_ok_and(|ip| ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified())
}

/// Adds a scheme when the user omitted one: http for localhost, https otherwise.
pub fn normalize_url(url: &str) -> String {
    let url = url.trim();
    if has_scheme(url) {
        return url.to_string();
    }
    if is_local_host(host_of(url)) {
        format!("http://{url}")
    } else {
        format!("https://{url}")
    }
}

pub fn prepare(req: &Request, vars_in: &HashMap<String, String>) -> Result<Prepared> {
    let mut missing: Vec<String> = Vec::new();
    let mut sub = |s: &str| -> String {
        let (out, m) = vars::interpolate(s, vars_in);
        for name in m {
            if !missing.contains(&name) {
                missing.push(name);
            }
        }
        out
    };

    let raw_url = sub(&req.url);
    if raw_url.trim().is_empty() {
        return Err(anyhow!("URL is empty"));
    }
    let mut url = reqwest::Url::parse(&normalize_url(&raw_url)).map_err(|e| anyhow!("Invalid URL: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(anyhow!("Unsupported URL scheme: {}", url.scheme()));
    }
    {
        let mut pairs = url.query_pairs_mut();
        for p in req.params.iter().filter(|p| p.enabled && !p.key.trim().is_empty()) {
            pairs.append_pair(&sub(&p.key), &sub(&p.value));
        }
    }
    if url.query() == Some("") {
        url.set_query(None);
    }

    let mut headers: Vec<(String, String)> = req
        .headers
        .iter()
        .filter(|h| h.enabled && !h.key.trim().is_empty())
        .map(|h| (sub(h.key.trim()), sub(&h.value)))
        .collect();

    // The Auth tab wins over a manually typed header of the same name.
    let drop_header = |headers: &mut Vec<(String, String)>, name: &str| {
        headers.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
    };
    let mut basic = None;
    let mut secret_header = None;
    match &req.auth {
        Auth::None => {}
        // An empty token/username means "not filled in yet": keep any manual header instead.
        Auth::Bearer { token } if !token.trim().is_empty() => {
            drop_header(&mut headers, "Authorization");
            headers.push(("Authorization".into(), format!("Bearer {}", sub(token).trim())));
        }
        Auth::Basic { username, password } if !username.is_empty() => {
            drop_header(&mut headers, "Authorization");
            basic = Some((sub(username), sub(password)));
        }
        Auth::ApiKey { name, value } if !name.trim().is_empty() => {
            let name = sub(name.trim());
            drop_header(&mut headers, &name);
            headers.push((name.clone(), sub(value)));
            secret_header = Some(name);
        }
        Auth::Bearer { .. } | Auth::Basic { .. } | Auth::ApiKey { .. } => {}
    }

    let body = match req.body_kind {
        BodyKind::None => PreparedBody::None,
        BodyKind::Raw => PreparedBody::Raw(sub(&req.body)),
        BodyKind::Form => PreparedBody::Form(
            req.form
                .iter()
                .filter(|f| f.enabled && !f.key.trim().is_empty())
                .map(|f| (sub(&f.key), sub(&f.value)))
                .collect(),
        ),
    };

    Ok(Prepared {
        method: req.method,
        url: url.to_string(),
        headers,
        basic,
        body,
        missing_vars: missing,
        secret_header,
    })
}

/// Variables used by the parts of the request that will actually be sent
/// (enabled rows, the active body kind), mirroring what `prepare` substitutes.
pub fn missing_vars(req: &Request, vars_in: &HashMap<String, String>) -> Vec<String> {
    collect_missing(req, vars_in, false)
}

/// Undefined variables in the URL, headers and auth. Sending those literally would hit the
/// wrong URL or leak a placeholder as a credential, so the app refuses to send.
pub fn critical_missing_vars(req: &Request, vars_in: &HashMap<String, String>) -> Vec<String> {
    collect_missing(req, vars_in, true)
}

fn collect_missing(req: &Request, vars_in: &HashMap<String, String>, critical_only: bool) -> Vec<String> {
    let mut missing: Vec<String> = Vec::new();
    let mut scan = |s: &str| {
        if !s.contains("{{") {
            return;
        }
        for name in vars::interpolate(s, vars_in).1 {
            if !missing.contains(&name) {
                missing.push(name);
            }
        }
    };
    let live = |kv: &&crate::model::KeyValue| kv.enabled && !kv.key.trim().is_empty();
    scan(&req.url);
    if !critical_only {
        for kv in req.params.iter().filter(live) {
            scan(&kv.key);
            scan(&kv.value);
        }
    }
    // Mirror `prepare`: a filled-in Auth tab replaces a manual header of the same name,
    // and an unfilled Bearer/Basic is ignored.
    let overridden: Option<String> = match &req.auth {
        Auth::Bearer { token } if !token.trim().is_empty() => Some("authorization".into()),
        Auth::Basic { username, .. } if !username.is_empty() => Some("authorization".into()),
        Auth::ApiKey { name, .. } if !name.trim().is_empty() => Some(name.trim().to_ascii_lowercase()),
        _ => None,
    };
    for kv in req.headers.iter().filter(live) {
        if overridden
            .as_deref()
            .is_some_and(|o| kv.key.trim().eq_ignore_ascii_case(o))
        {
            continue;
        }
        scan(&kv.key);
        scan(&kv.value);
    }
    match &req.auth {
        Auth::None => {}
        Auth::Bearer { token } if !token.trim().is_empty() => scan(token),
        Auth::Basic { username, password } if !username.is_empty() => {
            scan(username);
            scan(password);
        }
        Auth::Bearer { .. } | Auth::Basic { .. } => {}
        Auth::ApiKey { name, value } if !name.trim().is_empty() => {
            scan(name);
            scan(value);
        }
        Auth::ApiKey { .. } => {}
    }
    if critical_only {
        return missing;
    }
    match req.body_kind {
        BodyKind::None => {}
        BodyKind::Raw => scan(&req.body),
        BodyKind::Form => {
            for kv in req.form.iter().filter(live) {
                scan(&kv.key);
                scan(&kv.value);
            }
        }
    }
    missing
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Requests carrying credential-like headers use the client that refuses cross-origin redirects.
fn is_guarded(p: &Prepared) -> bool {
    p.secret_header.is_some() || p.headers.iter().any(|(k, _)| is_sensitive_header(k))
}

/// application/x-www-form-urlencoded escaping, as reqwest applies to form keys and values.
fn form_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | b'*' | b'-' | b'.' | b'_' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A copy-pasteable `curl` command equivalent to what `send` would do (variables already
/// resolved). Uses POSIX single-quote escaping, so it targets sh/bash/zsh, not cmd/PowerShell.
pub fn to_curl(p: &Prepared) -> String {
    // send() refuses cross-origin redirects for credential-like headers, so don't add -L then.
    let guarded = is_guarded(p);
    let mut parts: Vec<String> = vec!["curl".into()];
    if !guarded {
        parts.push("-L".into());
    }
    // `[]{}` in a URL would otherwise be treated as a curl glob.
    if p.url.contains(['[', ']', '{', '}']) {
        parts.push("-g".into());
    }
    // An all-disabled form emits no data flag, so it must not count as a body (curl would then
    // turn the request into a POST without any data).
    let has_body = match &p.body {
        PreparedBody::None => false,
        PreparedBody::Raw(_) => true,
        PreparedBody::Form(pairs) => !pairs.is_empty(),
    };
    match p.method {
        // curl turns any request with a body into a POST unless told otherwise.
        Method::Get if has_body => parts.push("-X GET".into()),
        Method::Get => {}
        // A body already makes curl POST, and then redirects behave like send() (302 -> GET).
        Method::Post if has_body => {}
        Method::Head if !has_body => parts.push("--head".into()),
        m => parts.push(format!("-X {}", m.as_str())),
    }
    parts.push(shell_quote(&p.url));
    let base = parts.len();
    for (k, v) in &p.headers {
        // curl drops `Name:` with no value; `Name;` is its syntax for sending an empty header.
        let header = if v.is_empty() {
            format!("{k};")
        } else {
            format!("{k}: {v}")
        };
        parts.push(format!("-H {}", shell_quote(&header)));
    }
    if let Some((u, pw)) = &p.basic {
        parts.push(format!("-u {}", shell_quote(&format!("{u}:{pw}"))));
    }
    match &p.body {
        PreparedBody::None => {}
        PreparedBody::Raw(s) => {
            let has_ct = p.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type"));
            if !has_ct && looks_like_json(s) {
                parts.push("-H 'Content-Type: application/json'".into());
            } else if !has_ct {
                // send() adds no Content-Type here, but curl would claim a form post.
                parts.push("-H 'Content-Type:'".into());
            }
            parts.push(format!("--data-raw {}", shell_quote(s)));
        }
        PreparedBody::Form(pairs) => {
            // curl encodes only the value, so pre-encode the key the way reqwest does.
            for (k, v) in pairs {
                parts.push(format!(
                    "--data-urlencode {}",
                    shell_quote(&format!("{}={v}", form_encode(k)))
                ));
            }
        }
    }
    if parts.len() == base {
        parts.join(" ")
    } else {
        parts.join(" \\\n  ")
    }
}

fn reqwest_method(m: Method) -> reqwest::Method {
    match m {
        Method::Get => reqwest::Method::GET,
        Method::Post => reqwest::Method::POST,
        Method::Put => reqwest::Method::PUT,
        Method::Patch => reqwest::Method::PATCH,
        Method::Delete => reqwest::Method::DELETE,
        Method::Head => reqwest::Method::HEAD,
        Method::Options => reqwest::Method::OPTIONS,
    }
}

fn looks_like_json(s: &str) -> bool {
    let t = s.trim_start();
    (t.starts_with('{') || t.starts_with('[')) && serde_json::from_str::<serde_json::Value>(s).is_ok()
}

/// Same origin for redirect purposes: same host, same port, and an http -> https upgrade
/// is allowed (a downgrade or any other scheme change is not).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn is_cross_origin(from: &reqwest::Url, to: &reqwest::Url) -> bool {
    if from.host_str() != to.host_str() {
        return true;
    }
    if from.scheme() == to.scheme() {
        return from.port_or_known_default() != to.port_or_known_default();
    }
    // Only the standard http (80) -> https (443) upgrade; custom ports may be different services.
    !(from.scheme() == "http"
        && to.scheme() == "https"
        && from.port_or_known_default() == Some(80)
        && to.port_or_known_default() == Some(443))
}

#[cfg(not(target_arch = "wasm32"))]
fn build_client(guarded: bool) -> Result<reqwest::Client> {
    let policy = reqwest::redirect::Policy::custom(move |attempt| {
        // `previous()` includes the original request, so 10 redirects means len() == 11.
        if attempt.previous().len() > 10 {
            return attempt.error("too many redirects");
        }
        let cross = attempt.previous().iter().any(|p| is_cross_origin(p, attempt.url()));
        if guarded && cross {
            attempt.stop()
        } else {
            attempt.follow()
        }
    });
    Ok(reqwest::Client::builder()
        .redirect(policy)
        .user_agent(concat!("postrust/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

/// In a browser `fetch` does the connecting, redirecting and compression itself, so there is
/// nothing to configure (and no way to refuse cross-origin redirects).
#[cfg(target_arch = "wasm32")]
fn build_client(_guarded: bool) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().build()?)
}

/// Shared HTTP clients so connection pools and TLS config are reused between requests.
/// reqwest already strips Authorization/Cookie on cross-origin hops; custom credential
/// headers (API key, X-Auth-Token, ...) would be forwarded, so requests carrying them use
/// the `guarded` client, which does not follow cross-origin redirects.
pub struct Http {
    plain: reqwest::Client,
    guarded: reqwest::Client,
}

impl Http {
    pub fn new() -> Result<Self> {
        Ok(Self {
            plain: build_client(false)?,
            guarded: build_client(true)?,
        })
    }

    pub async fn send(&self, p: Prepared, timeout: Duration) -> Result<ResponseData> {
        let guarded = is_guarded(&p);
        send_with(if guarded { &self.guarded } else { &self.plain }, p, timeout).await
    }
}

/// One-off convenience (builds fresh clients); the app uses a shared [`Http`].
pub async fn send(p: Prepared, timeout: Duration) -> Result<ResponseData> {
    Http::new()?.send(p, timeout).await
}

async fn send_with(client: &reqwest::Client, p: Prepared, timeout: Duration) -> Result<ResponseData> {
    let mut rb = client.request(reqwest_method(p.method), &p.url).timeout(timeout);
    let user_set_content_type = p.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type"));
    for (k, v) in &p.headers {
        rb = rb.header(k.as_str(), v.as_str());
    }
    if let Some((u, pw)) = &p.basic {
        rb = rb.basic_auth(u, Some(pw));
    }
    rb = match &p.body {
        PreparedBody::None => rb,
        PreparedBody::Raw(s) => {
            let rb = if !user_set_content_type && looks_like_json(s) {
                rb.header("Content-Type", "application/json")
            } else {
                rb
            };
            rb.body(s.clone())
        }
        PreparedBody::Form(pairs) => rb.form(pairs),
    };

    let start = Instant::now();
    let resp = rb.send().await.map_err(describe_error)?;
    let status = resp.status();
    let headers = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned()))
        .collect();

    let mut body = Vec::new();
    let mut size = 0usize;
    let mut truncated = false;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| stream_error(e, start.elapsed(), timeout))?;
        size += chunk.len();
        let room = MAX_BODY_BYTES - body.len();
        if chunk.len() > room {
            // Stop downloading; `size` is then a lower bound.
            body.extend_from_slice(&chunk[..room]);
            truncated = true;
            break;
        }
        body.extend_from_slice(&chunk);
    }
    if truncated {
        trim_partial_utf8(&mut body);
    }

    Ok(ResponseData {
        status: status.as_u16(),
        reason: status.canonical_reason().unwrap_or("").to_string(),
        elapsed_ms: start.elapsed().as_millis(),
        size,
        truncated,
        headers,
        body,
    })
}

/// File extension for a saved body, from the (case-insensitive) media type.
pub fn extension_for(content_type: Option<&str>) -> &'static str {
    let ct = content_type.unwrap_or("").to_ascii_lowercase();
    match ct.split(';').next().unwrap_or("").trim() {
        "image/svg+xml" => "svg",
        "text/html" => "html",
        "text/csv" => "csv",
        "text/plain" => "txt",
        "application/javascript" | "text/javascript" => "js",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "application/pdf" => "pdf",
        "application/zip" => "zip",
        "application/gzip" | "application/x-gzip" => "gz",
        m if m.ends_with("json") => "json",
        m if m.ends_with("xml") => "xml",
        m if m.starts_with("text/") => "txt",
        _ => "bin",
    }
}

/// Enabled header rows whose name satisfies `pred`, as written, once each (case-insensitive).
fn unique_enabled_header_names(rows: &[crate::model::KeyValue], pred: impl Fn(&str) -> bool) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for row in rows.iter().filter(|r| r.enabled) {
        let name = row.key.trim();
        if pred(name) && !found.iter().any(|f| f.eq_ignore_ascii_case(name)) {
            found.push(name.to_string());
        }
    }
    found
}

/// The Fetch spec's "forbidden header names", including any `Proxy-` or `Sec-` header.
fn is_browser_forbidden_header(name: &str) -> bool {
    const FORBIDDEN: &[&str] = &[
        "accept-charset",
        "accept-encoding",
        "access-control-request-headers",
        "access-control-request-method",
        "connection",
        "content-length",
        "cookie",
        "cookie2",
        "date",
        "dnt",
        "expect",
        "host",
        "keep-alive",
        "origin",
        "referer",
        "set-cookie",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
        "via",
    ];
    let lower = name.to_ascii_lowercase();
    FORBIDDEN.contains(&lower.as_str()) || lower.starts_with("proxy-") || lower.starts_with("sec-")
}

/// Request headers a browser refuses to let a page set. They are silently dropped by `fetch`, so
/// the web UI warns about them. Names as written, once each, for enabled rows only.
pub fn browser_forbidden_headers(rows: &[crate::model::KeyValue]) -> Vec<String> {
    unique_enabled_header_names(rows, is_browser_forbidden_header)
}

/// Enabled header rows that look like credentials (API keys, tokens...). A browser follows
/// redirects by itself, so on the web these could reach another site; the UI warns about them.
pub fn credential_header_names(rows: &[crate::model::KeyValue]) -> Vec<String> {
    unique_enabled_header_names(rows, is_sensitive_header)
}

/// The custom header the Auth tab adds (an API key), as a row, so the header checks above can
/// cover it too. Bearer and Basic use `Authorization`, which browsers already strip on a
/// cross-origin redirect.
pub fn auth_header_rows(auth: &Auth) -> Vec<crate::model::KeyValue> {
    match auth {
        Auth::ApiKey { name, value } if !name.trim().is_empty() => vec![crate::model::KeyValue::new(name, value)],
        _ => Vec::new(),
    }
}

/// Credential-looking custom headers. Authorization/Cookie are excluded because reqwest
/// already strips those itself when a redirect leaves the origin.
fn is_sensitive_header(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    if matches!(n.as_str(), "authorization" | "proxy-authorization" | "cookie") {
        return false;
    }
    ["key", "token", "secret", "auth", "session", "password", "credential"]
        .iter()
        .any(|w| n.contains(w))
}

/// Drops a multi-byte character cut in half by truncation so text isn't mistaken for binary.
pub fn trim_partial_utf8(body: &mut Vec<u8>) {
    if let Err(e) = std::str::from_utf8(body)
        && e.error_len().is_none()
    {
        body.truncate(e.valid_up_to());
    }
}

/// reqwest's Display omits the underlying cause (DNS, TLS, refused...), so walk the chain.
#[cfg(not(target_arch = "wasm32"))]
fn error_chain(e: &reqwest::Error) -> String {
    let mut msg = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        let part = s.to_string();
        if !msg.contains(&part) {
            msg.push_str(": ");
            msg.push_str(&part);
        }
        src = s.source();
    }
    msg
}

/// Whether a request has used up its timeout (with a little slack for scheduling).
fn timed_out(elapsed: Duration, timeout: Duration) -> bool {
    elapsed + Duration::from_millis(100) >= timeout
}

/// An error while the body is streaming. The browser backend reports a timeout that fires
/// mid-stream as an "error decoding response body", so the clock decides what to tell the user.
fn stream_error(e: reqwest::Error, elapsed: Duration, timeout: Duration) -> anyhow::Error {
    if timed_out(elapsed, timeout) {
        anyhow!("Request timed out")
    } else {
        describe_error(e)
    }
}

/// What the web build says when `fetch` fails. Browsers deliberately hide the reason (a CORS
/// block looks exactly like an unreachable server), so this lists the usual causes instead of
/// showing the useless raw JavaScript error and stack trace.
pub fn browser_request_failed_message() -> &'static str {
    "The browser could not complete this request. Common causes:\n\
     - The server does not send CORS headers (Access-Control-Allow-Origin). Browsers only let a \
     web page call other servers that allow it.\n\
     - The address is wrong, or the server is not running.\n\
     - This page is https but the address is http (blocked as mixed content; http://localhost is \
     allowed).\n\
     Browsers do not say which one it was, so check the server's CORS headers and the browser's \
     developer console."
}

#[cfg(target_arch = "wasm32")]
fn describe_error(e: reqwest::Error) -> anyhow::Error {
    if e.is_timeout() {
        anyhow!("Request timed out")
    } else if e.is_builder() || e.is_body() || e.is_decode() {
        // These are not the opaque network failure: keep the real cause (first line only, the
        // browser appends a JavaScript stack trace).
        let text = e.to_string();
        anyhow!("{}", text.lines().next().unwrap_or("The request could not be built"))
    } else {
        anyhow!("{}", browser_request_failed_message())
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn describe_error(e: reqwest::Error) -> anyhow::Error {
    if e.is_timeout() {
        anyhow!("Request timed out")
    } else if e.is_connect() {
        anyhow!("Connection failed: {}", error_chain(&e))
    } else if e.is_redirect() {
        anyhow!("Too many redirects")
    } else {
        anyhow!("{}", error_chain(&e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::KeyValue;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn normalizes_scheme() {
        assert_eq!(normalize_url("example.com/a"), "https://example.com/a");
        assert_eq!(normalize_url("localhost:3000/x"), "http://localhost:3000/x");
        assert_eq!(normalize_url("127.0.0.1"), "http://127.0.0.1");
        assert_eq!(normalize_url("192.168.1.20:8080/api"), "http://192.168.1.20:8080/api");
        assert_eq!(normalize_url("10.0.0.5"), "http://10.0.0.5");
        assert_eq!(normalize_url("0.0.0.0:3000"), "http://0.0.0.0:3000");
        assert_eq!(normalize_url("myhost.local:8000"), "http://myhost.local:8000");
        assert_eq!(normalize_url("8.8.8.8/x"), "https://8.8.8.8/x");
        assert_eq!(normalize_url("172.32.0.1"), "https://172.32.0.1", "outside 172.16/12");
        assert_eq!(normalize_url("http://a.b"), "http://a.b");
    }

    #[test]
    fn scheme_detection_ignores_embedded_urls() {
        assert_eq!(
            normalize_url("example.com/cb?next=https://foo.com"),
            "https://example.com/cb?next=https://foo.com"
        );
        assert_eq!(
            normalize_url("localhost:3000/r?u=http://x"),
            "http://localhost:3000/r?u=http://x"
        );
        assert_eq!(normalize_url("[::1]:8080/x"), "http://[::1]:8080/x");
        assert_eq!(normalize_url("LOCALHOST:3000"), "http://LOCALHOST:3000");
        assert_eq!(
            normalize_url("user:pw@localhost:3000/a"),
            "http://user:pw@localhost:3000/a"
        );
        assert_eq!(normalize_url("HTTP://a.b"), "HTTP://a.b");
    }

    #[test]
    fn auth_tab_replaces_manual_header() {
        let mut r = Request::new("x");
        r.url = "http://h.test".into();
        r.headers = vec![KeyValue::new("authorization", "manual"), KeyValue::new("X-Other", "1")];
        r.auth = Auth::Bearer { token: "t".into() };
        let p = prepare(&r, &vars(&[])).unwrap();
        let auth: Vec<_> = p
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("authorization"))
            .collect();
        assert_eq!(auth.len(), 1);
        assert_eq!(auth[0].1, "Bearer t");
        assert!(p.headers.iter().any(|(k, _)| k == "X-Other"));
    }

    #[test]
    fn missing_vars_ignores_disabled_and_inactive_body() {
        let mut r = Request::new("{{in_name}}");
        r.url = "{{a}}".into();
        let mut off = KeyValue::new("X", "{{off}}");
        off.enabled = false;
        r.headers = vec![off, KeyValue::new("Y", "{{b}}")];
        r.body = "{{in_body}}".into();
        assert_eq!(missing_vars(&r, &vars(&[])), vec!["a", "b"]);
        r.body_kind = BodyKind::Raw;
        assert_eq!(missing_vars(&r, &vars(&[("a", "1")])), vec!["b", "in_body"]);
    }

    #[test]
    fn missing_vars_matches_prepare() {
        let mut r = Request::new("x");
        r.url = "http://h/{{u}}".into();
        r.params = vec![KeyValue::new("{{pk}}", "{{pv}}")];
        r.headers = vec![KeyValue::new("H", "{{h}}")];
        r.auth = Auth::ApiKey {
            name: "".into(),
            value: "{{unused}}".into(),
        };
        r.body_kind = BodyKind::Form;
        r.form = vec![KeyValue::new("f", "{{fv}}")];
        r.body = "{{ignored}}".into();
        let v = vars(&[]);
        assert_eq!(missing_vars(&r, &v), prepare(&r, &v).unwrap().missing_vars);
        r.auth = Auth::Basic {
            username: "{{bu}}".into(),
            password: "{{bp}}".into(),
        };
        assert_eq!(missing_vars(&r, &v), prepare(&r, &v).unwrap().missing_vars);
    }

    #[test]
    fn decodes_declared_charset() {
        let r = ResponseData {
            status: 200,
            reason: "OK".into(),
            elapsed_ms: 0,
            size: 4,
            truncated: false,
            headers: vec![("Content-Type".into(), "text/html; charset=ISO-8859-1".into())],
            body: vec![b'c', b'a', b'f', 0xe9],
        };
        assert!(r.is_text());
        assert_eq!(r.text(), "café");
    }

    #[test]
    fn cross_origin_rules() {
        let u = |s: &str| reqwest::Url::parse(s).unwrap();
        assert!(
            !is_cross_origin(&u("http://a.com/x"), &u("https://a.com/y")),
            "upgrade is fine"
        );
        assert!(
            is_cross_origin(&u("https://a.com/x"), &u("http://a.com/y")),
            "downgrade"
        );
        assert!(is_cross_origin(&u("https://a.com"), &u("https://b.com")));
        assert!(is_cross_origin(&u("http://a.com:1"), &u("http://a.com:2")));
        assert!(
            is_cross_origin(&u("http://a.com:8080"), &u("https://a.com:8443")),
            "custom-port upgrade"
        );
        assert!(is_cross_origin(&u("http://a.com"), &u("https://a.com:8443")));
        assert!(!is_cross_origin(&u("http://a.com/p"), &u("http://a.com/q")));
    }

    #[test]
    fn extensions_ignore_case_and_params() {
        assert_eq!(extension_for(Some("Application/JSON; charset=utf-8")), "json");
        assert_eq!(extension_for(Some("TEXT/HTML")), "html");
        assert_eq!(extension_for(Some("image/svg+xml")), "svg");
        assert_eq!(extension_for(Some("application/vnd.api+json")), "json");
        assert_eq!(extension_for(Some("text/csv")), "csv");
        assert_eq!(extension_for(Some("image/png")), "png");
        assert_eq!(extension_for(Some("application/pdf")), "pdf");
        assert_eq!(extension_for(Some("application/octet-stream")), "bin");
        assert_eq!(extension_for(None), "bin");
    }

    #[tokio::test]
    async fn connection_errors_include_the_cause() {
        let mut r = Request::new("x");
        r.url = "http://127.0.0.1:1".into();
        let err = send(prepare(&r, &vars(&[])).unwrap(), Duration::from_secs(2))
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.starts_with("Connection failed: "), "{msg}");
        assert!(msg.matches(": ").count() >= 2, "expected an underlying cause in: {msg}");
    }

    #[test]
    fn curl_for_a_simple_get_is_one_line() {
        let mut r = Request::new("x");
        r.url = "http://h.test/a b".into();
        let p = prepare(&r, &vars(&[])).unwrap();
        assert_eq!(to_curl(&p), "curl -L 'http://h.test/a%20b'");
    }

    #[test]
    fn curl_covers_method_headers_auth_and_json_body() {
        let mut r = Request::new("x");
        r.method = Method::Post;
        r.url = "http://h.test".into();
        r.headers = vec![KeyValue::new("X-Id", "7")];
        r.auth = Auth::Basic {
            username: "u".into(),
            password: "p w".into(),
        };
        r.body_kind = BodyKind::Raw;
        r.body = r#"{"a":"it's"}"#.into();
        let curl = to_curl(&prepare(&r, &vars(&[])).unwrap());
        assert!(curl.starts_with("curl \\\n  -L \\\n  'http://h.test/'"), "{curl}");
        assert!(!curl.contains("-X POST"), "a body already makes curl POST: {curl}");
        assert!(curl.contains("-H 'X-Id: 7'"), "{curl}");
        assert!(curl.contains("-u 'u:p w'"), "{curl}");
        assert!(curl.contains("-H 'Content-Type: application/json'"), "{curl}");
        assert!(curl.contains(r#"--data-raw '{"a":"it'\''s"}'"#), "{curl}");
    }

    #[test]
    fn curl_empty_form_and_empty_header_values() {
        let mut r = Request::new("x");
        r.method = Method::Post;
        r.url = "http://h.test".into();
        r.body_kind = BodyKind::Form;
        let mut off = KeyValue::new("a", "1");
        off.enabled = false;
        r.form = vec![off];
        r.headers = vec![KeyValue::new("X-Empty", "")];
        let curl = to_curl(&prepare(&r, &vars(&[])).unwrap());
        assert!(
            curl.contains("-X POST"),
            "no data is emitted, so POST must be explicit: {curl}"
        );
        assert!(curl.contains("-H 'X-Empty;'"), "{curl}");
        assert!(!curl.contains("X-Empty:"), "{curl}");
    }

    #[test]
    fn curl_raw_text_body_does_not_claim_to_be_a_form() {
        let mut r = Request::new("x");
        r.method = Method::Post;
        r.url = "http://h.test".into();
        r.body_kind = BodyKind::Raw;
        r.body = "<xml/>".into();
        let curl = to_curl(&prepare(&r, &vars(&[])).unwrap());
        assert!(
            curl.contains("-H 'Content-Type:'"),
            "must suppress curl's default: {curl}"
        );
        r.headers = vec![KeyValue::new("Content-Type", "application/xml")];
        let curl = to_curl(&prepare(&r, &vars(&[])).unwrap());
        assert!(!curl.contains("-H 'Content-Type:'"), "{curl}");
        assert!(curl.contains("Content-Type: application/xml"), "{curl}");
    }

    #[test]
    fn curl_methods_that_need_an_explicit_flag() {
        let mut r = Request::new("x");
        r.url = "http://h.test".into();
        r.method = Method::Post; // no body: curl would send GET without -X
        assert!(to_curl(&prepare(&r, &vars(&[])).unwrap()).contains("-X POST"));
        for m in [Method::Put, Method::Patch, Method::Delete, Method::Options] {
            r.method = m;
            r.body_kind = BodyKind::Raw;
            r.body = "{}".into();
            assert!(to_curl(&prepare(&r, &vars(&[])).unwrap()).contains(&format!("-X {}", m.as_str())));
        }
        r.method = Method::Head;
        let curl = to_curl(&prepare(&r, &vars(&[])).unwrap());
        assert!(!curl.contains("--head"), "--head conflicts with a body: {curl}");
        assert!(curl.contains("-X HEAD"), "{curl}");
    }

    #[test]
    fn curl_globoff_for_brackets_and_no_redirects_for_guarded_requests() {
        let mut r = Request::new("x");
        r.url = "http://h.test/items?filter[name]=x".into();
        assert!(to_curl(&prepare(&r, &vars(&[])).unwrap()).contains("-g"));
        r.url = "http://h.test/plain".into();
        assert!(!to_curl(&prepare(&r, &vars(&[])).unwrap()).contains("-g"));

        assert!(to_curl(&prepare(&r, &vars(&[])).unwrap()).contains("-L"));
        r.auth = Auth::ApiKey {
            name: "X-Key".into(),
            value: "k".into(),
        };
        assert!(
            !to_curl(&prepare(&r, &vars(&[])).unwrap()).contains("-L"),
            "guarded: no cross-origin redirects"
        );
        r.auth = Auth::None;
        r.headers = vec![KeyValue::new("X-Auth-Token", "t")];
        assert!(!to_curl(&prepare(&r, &vars(&[])).unwrap()).contains("-L"));
        r.headers = vec![KeyValue::new("Authorization", "Bearer t")];
        assert!(
            to_curl(&prepare(&r, &vars(&[])).unwrap()).contains("-L"),
            "reqwest strips Authorization itself"
        );
    }

    #[test]
    fn curl_form_keys_are_encoded_like_reqwest() {
        assert_eq!(form_encode("user name"), "user+name");
        assert_eq!(form_encode("a&b=c"), "a%26b%3Dc");
        assert_eq!(form_encode("é-._*"), "%C3%A9-._*");
        let mut r = Request::new("x");
        r.url = "http://h.test".into();
        r.method = Method::Post;
        r.body_kind = BodyKind::Form;
        r.form = vec![KeyValue::new("user name", "a b")];
        let curl = to_curl(&prepare(&r, &vars(&[])).unwrap());
        assert!(curl.contains("--data-urlencode 'user+name=a b'"), "{curl}");
    }

    #[test]
    fn curl_get_with_a_body_keeps_the_get_method() {
        let mut r = Request::new("x");
        r.url = "http://h.test".into();
        r.body_kind = BodyKind::Raw;
        r.body = "q".into();
        let curl = to_curl(&prepare(&r, &vars(&[])).unwrap());
        assert!(curl.contains("-X GET"), "curl would otherwise send POST: {curl}");
        r.body_kind = BodyKind::None;
        assert!(!to_curl(&prepare(&r, &vars(&[])).unwrap()).contains("-X"));
    }

    #[test]
    fn curl_form_and_head_and_explicit_content_type() {
        let mut r = Request::new("x");
        r.method = Method::Put;
        r.url = "http://h.test".into();
        r.body_kind = BodyKind::Form;
        r.form = vec![KeyValue::new("a", "1"), KeyValue::new("b", "x y")];
        let curl = to_curl(&prepare(&r, &vars(&[])).unwrap());
        assert!(
            curl.contains("--data-urlencode 'a=1'") && curl.contains("--data-urlencode 'b=x y'"),
            "{curl}"
        );

        r.method = Method::Head;
        r.body_kind = BodyKind::None;
        assert!(to_curl(&prepare(&r, &vars(&[])).unwrap()).contains("--head"));

        r.method = Method::Post;
        r.headers = vec![KeyValue::new("content-type", "text/plain")];
        r.body_kind = BodyKind::Raw;
        r.body = "{}".into();
        let curl = to_curl(&prepare(&r, &vars(&[])).unwrap());
        assert!(!curl.contains("application/json"), "user content-type wins: {curl}");
    }

    #[test]
    fn browser_forbidden_headers_are_found_once_and_only_when_enabled() {
        let mut off = KeyValue::new("Origin", "x");
        off.enabled = false;
        let rows = vec![
            KeyValue::new("Cookie", "a=1"),
            KeyValue::new("cookie", "b=2"),
            KeyValue::new("Host", "example.com"),
            KeyValue::new("Sec-Fetch-Mode", "cors"),
            KeyValue::new("Proxy-Authorization", "x"),
            KeyValue::new("X-Custom", "ok"),
            KeyValue::new("Authorization", "Bearer t"),
            KeyValue::new("User-Agent", "postrust"),
            KeyValue::new("", "blank key"),
            off,
        ];
        assert_eq!(
            browser_forbidden_headers(&rows),
            vec!["Cookie", "Host", "Sec-Fetch-Mode", "Proxy-Authorization"]
        );
        assert!(browser_forbidden_headers(&[]).is_empty());
    }

    #[test]
    fn a_request_that_used_its_whole_timeout_counts_as_timed_out() {
        let t = Duration::from_secs(1);
        assert!(!timed_out(Duration::from_millis(100), t));
        assert!(!timed_out(Duration::from_millis(850), t));
        assert!(
            timed_out(Duration::from_millis(900), t),
            "within scheduling slack of the limit"
        );
        assert!(timed_out(Duration::from_millis(1000), t));
        assert!(timed_out(Duration::from_secs(5), t));
        assert!(
            timed_out(Duration::from_millis(0), Duration::from_millis(50)),
            "tiny timeouts"
        );
    }

    #[test]
    fn the_browser_failure_message_explains_the_usual_causes_without_js_noise() {
        let msg = browser_request_failed_message();
        for needle in [
            "CORS",
            "Access-Control-Allow-Origin",
            "mixed content",
            "localhost",
            "developer console",
        ] {
            assert!(msg.contains(needle), "{needle} missing: {msg}");
        }
        for noise in ["JsValue", "wasm-function", "TypeError", "at http"] {
            assert!(!msg.contains(noise), "{noise} must not leak into the message");
        }
    }

    #[test]
    fn the_auth_tab_api_key_is_visible_to_the_header_checks() {
        let key = Auth::ApiKey {
            name: "X-Api-Key".into(),
            value: "k".into(),
        };
        let rows = auth_header_rows(&key);
        assert_eq!(rows.len(), 1);
        assert_eq!(credential_header_names(&rows), vec!["X-Api-Key"]);
        assert!(browser_forbidden_headers(&rows).is_empty());
        // An API key named like a forbidden header is caught too.
        let cookie = Auth::ApiKey {
            name: "Cookie".into(),
            value: "k".into(),
        };
        assert_eq!(browser_forbidden_headers(&auth_header_rows(&cookie)), vec!["Cookie"]);
        // Nothing for the other kinds (Authorization is stripped by browsers on redirects).
        for auth in [
            Auth::None,
            Auth::Bearer { token: "t".into() },
            Auth::Basic {
                username: "u".into(),
                password: "p".into(),
            },
            Auth::ApiKey {
                name: "  ".into(),
                value: "k".into(),
            },
        ] {
            assert!(auth_header_rows(&auth).is_empty(), "{auth:?}");
        }
    }

    #[test]
    fn credential_header_names_lists_enabled_credential_like_rows_once() {
        let mut off = KeyValue::new("X-Secret", "x");
        off.enabled = false;
        let rows = vec![
            KeyValue::new("X-API-Key", "k"),
            KeyValue::new("x-api-key", "k2"),
            KeyValue::new("X-Auth-Token", "t"),
            KeyValue::new("Authorization", "Bearer t"),
            KeyValue::new("X-Trace", "1"),
            off,
        ];
        assert_eq!(credential_header_names(&rows), vec!["X-API-Key", "X-Auth-Token"]);
    }

    #[test]
    fn sensitive_header_heuristic() {
        for h in ["X-API-Key", "x-auth-token", "X-Session-Id", "Client-Secret"] {
            assert!(is_sensitive_header(h), "{h}");
        }
        for h in ["Authorization", "Cookie", "X-Trace", "Accept", "Content-Type"] {
            assert!(!is_sensitive_header(h), "{h}");
        }
    }

    #[test]
    fn bearer_token_whitespace_is_trimmed() {
        let mut r = Request::new("x");
        r.url = "http://h.test".into();
        r.auth = Auth::Bearer { token: " abc\n".into() };
        let p = prepare(&r, &vars(&[])).unwrap();
        assert_eq!(p.headers, vec![("Authorization".to_string(), "Bearer abc".to_string())]);
    }

    #[test]
    fn empty_bearer_keeps_manual_authorization() {
        let mut r = Request::new("x");
        r.url = "http://h.test".into();
        r.headers = vec![KeyValue::new("Authorization", "Token xyz")];
        r.auth = Auth::Bearer { token: "  ".into() };
        let p = prepare(&r, &vars(&[])).unwrap();
        assert_eq!(p.headers, vec![("Authorization".to_string(), "Token xyz".to_string())]);
    }

    #[test]
    fn critical_missing_covers_url_headers_auth_only() {
        let mut r = Request::new("x");
        r.url = "{{base}}".into();
        r.params = vec![KeyValue::new("p", "{{param}}")];
        r.headers = vec![KeyValue::new("H", "{{hdr}}")];
        r.auth = Auth::Bearer {
            token: "{{tok}}".into(),
        };
        r.body_kind = BodyKind::Raw;
        r.body = "{{body}}".into();
        assert_eq!(critical_missing_vars(&r, &vars(&[])), vec!["base", "hdr", "tok"]);
        assert_eq!(
            missing_vars(&r, &vars(&[])),
            vec!["base", "param", "hdr", "tok", "body"]
        );
    }

    #[test]
    fn critical_missing_ignores_overridden_and_unfilled_auth() {
        let mut r = Request::new("x");
        r.url = "http://h".into();
        r.headers = vec![KeyValue::new("authorization", "{{old}}")];
        r.auth = Auth::Bearer { token: "real".into() };
        assert!(critical_missing_vars(&r, &vars(&[])).is_empty());
        r.auth = Auth::Basic {
            username: "".into(),
            password: "{{pw}}".into(),
        };
        assert_eq!(critical_missing_vars(&r, &vars(&[])), vec!["old"]);
        assert_eq!(
            missing_vars(&r, &vars(&[])),
            prepare(&r, &vars(&[])).unwrap().missing_vars
        );
    }

    #[test]
    fn trims_cut_utf8() {
        let mut b = "aé".as_bytes().to_vec();
        b.pop();
        trim_partial_utf8(&mut b);
        assert_eq!(b, b"a");
        let mut bin = vec![0xff, 0x00];
        trim_partial_utf8(&mut bin);
        assert_eq!(bin, vec![0xff, 0x00]);
    }

    #[test]
    fn merges_params_and_vars() {
        let mut r = Request::new("x");
        r.url = "{{host}}/search?a=1".into();
        r.params = vec![KeyValue::new("q", "hello world"), {
            let mut d = KeyValue::new("off", "1");
            d.enabled = false;
            d
        }];
        let p = prepare(&r, &vars(&[("host", "http://h.test")])).unwrap();
        assert_eq!(p.url, "http://h.test/search?a=1&q=hello+world");
    }

    #[test]
    fn reports_missing_vars() {
        let mut r = Request::new("x");
        r.url = "http://h.test/{{nope}}".into();
        let p = prepare(&r, &vars(&[])).unwrap();
        assert_eq!(p.missing_vars, vec!["nope"]);
    }

    #[test]
    fn rejects_bad_urls() {
        let mut r = Request::new("x");
        assert!(prepare(&r, &vars(&[])).is_err());
        r.url = "ftp://x.y".into();
        assert!(prepare(&r, &vars(&[])).is_err());
        r.url = "http://".into();
        assert!(prepare(&r, &vars(&[])).is_err());
    }

    #[test]
    fn auth_variants() {
        let mut r = Request::new("x");
        r.url = "http://h.test".into();
        r.auth = Auth::Bearer { token: "{{t}}".into() };
        let p = prepare(&r, &vars(&[("t", "abc")])).unwrap();
        assert!(p.headers.contains(&("Authorization".into(), "Bearer abc".into())));
        r.auth = Auth::Basic {
            username: "u".into(),
            password: "p".into(),
        };
        assert_eq!(prepare(&r, &vars(&[])).unwrap().basic, Some(("u".into(), "p".into())));
        r.auth = Auth::ApiKey {
            name: "X-Key".into(),
            value: "k".into(),
        };
        assert!(
            prepare(&r, &vars(&[]))
                .unwrap()
                .headers
                .contains(&("X-Key".into(), "k".into()))
        );
    }

    #[test]
    fn pretty_prints_json() {
        let r = ResponseData {
            status: 200,
            reason: "OK".into(),
            elapsed_ms: 1,
            size: 7,
            truncated: false,
            headers: vec![],
            body: br#"{"a":1}"#.to_vec(),
        };
        assert_eq!(pretty_json(&r.text()).unwrap(), "{\n  \"a\": 1\n}");
        assert!(pretty_json("<html>").is_none());
        assert!(pretty_json("42").is_none());
        assert!(pretty_json("true").is_none());
        assert!(pretty_json("\"s\"").is_none());
    }
}
