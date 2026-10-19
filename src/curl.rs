//! Turns a pasted `curl` command line into a [`Request`].
//!
//! This is text parsing only: nothing is executed and no file is ever read (`-d @file` and
//! friends are reported as unsupported). The grammar targets what people actually paste: bash/zsh
//! style commands, including Chrome's and Firefox's "Copy as cURL" (line continuations and
//! `$'...'` quoting). Windows `cmd` style (`^` escapes) is not supported.

use crate::model::{Auth, BodyKind, KeyValue, Method, Request};

/// Pasted text longer than this is refused rather than parsed.
const MAX_LEN: usize = 1_000_000;

#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub request: Request,
    /// Things that were ignored or could not be represented, for the user to see.
    pub warnings: Vec<String>,
}

/// True when the text is a `curl ...` command (what the URL field looks for on paste).
pub fn looks_like_curl(text: &str) -> bool {
    let t = text.trim_start();
    let Some(rest) = t.strip_prefix("curl") else {
        return false;
    };
    let rest = rest.strip_prefix(".exe").unwrap_or(rest);
    if !rest.starts_with(char::is_whitespace) {
        return false;
    }
    // Prose like "curl is a command line tool" must not count: the first word after `curl` has
    // to be an option, a quoted/escaped argument, or something that looks like a URL.
    let mut after = rest.trim_start();
    while let Some(next) = after.strip_prefix("\\\n").or_else(|| after.strip_prefix("\\\r\n")) {
        after = next.trim_start();
    }
    let first = after.split_whitespace().next().unwrap_or("");
    first.starts_with('-') || first.starts_with(['\'', '"', '$', '^']) || looks_like_url(first)
}

type Words = std::iter::Peekable<std::vec::IntoIter<String>>;

/// Chrome's "Copy as cURL (cmd)" quotes with `^"` and continues lines with a trailing `^`, which
/// the bash-style tokenizer would turn into a garbage request.
fn looks_like_cmd_style(text: &str) -> bool {
    let t = text.trim_start();
    let t = t.strip_prefix("curl").unwrap_or(t);
    let t = t.strip_prefix(".exe").unwrap_or(t).trim_start();
    // "Copy as cURL (cmd)" always starts with `curl ^"URL^"` and continues lines with a trailing
    // " ^", where bash uses a trailing backslash. Quotes inside the body don't change that.
    t.starts_with("^\"") || (text.lines().any(|l| l.trim_end().ends_with(" ^")) && !text.contains("\\\n"))
}

/// Whether a token is plausibly the request URL rather than the value of an unknown option.
fn looks_like_url(s: &str) -> bool {
    if s.contains("://") || s.starts_with("localhost") {
        return true;
    }
    // A dotted host name ("example.com/x"), but not a number like "1.5m".
    let host = s.split(['/', ':', '?']).next().unwrap_or("");
    let last = host.rsplit('.').next().unwrap_or("");
    let dotted_name = host.contains('.') && last.chars().next().is_some_and(char::is_alphabetic);
    // A service name with a port ("api:8080/health").
    let service_port = s.split_once(':').is_some_and(|(h, rest)| {
        !h.is_empty()
            && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            && rest.chars().next().is_some_and(|c| c.is_ascii_digit())
            && rest.chars().take_while(|c| *c != '/').all(|c| c.is_ascii_digit())
    });
    // A bare IPv4 address ("127.0.0.1:8080").
    let ipv4 = host.split('.').count() == 4
        && host
            .split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    !s.contains(char::is_whitespace) && (dotted_name || ipv4 || service_port)
}

pub fn parse(text: &str) -> Result<Imported, String> {
    if text.len() > MAX_LEN {
        return Err("the pasted text is too large".into());
    }
    if looks_like_cmd_style(text) {
        return Err(
            "this looks like a Windows cmd-style command (with ^ escapes), which isn't supported; \
             use \"Copy as cURL (bash)\""
                .into(),
        );
    }
    let tokens = tokenize(text)?;
    let mut it: Words = tokens.words.into_iter().peekable();
    match it.next().as_deref() {
        Some("curl" | "curl.exe") => {}
        _ => return Err("this is not a curl command".into()),
    }
    let mut state = State::default();
    for name in &tokens.shell_vars {
        state.warn(format!(
            "Shell variable ${name} became {{{{{name}}}}}: define it in an environment"
        ));
    }
    if tokens.substitution {
        state.warn("Command substitution ($(...) or backticks) isn't evaluated; it was left as text");
    }
    if tokens.second_command {
        state.warn("Only the first curl command was imported; the next one was ignored");
    }
    if let Some(op) = tokens.stopped_at {
        state.warn(format!("Everything after `{op}` was ignored"));
    }
    if tokens.redirected {
        state.warn("Shell redirections (> and <) were ignored");
    }
    while let Some(word) = it.next() {
        if word == "--" {
            for rest in it.by_ref() {
                state.set_url(rest);
            }
            break;
        } else if let Some(long) = word.strip_prefix("--") {
            state.long_option(long, &mut it)?;
        } else if word.len() > 1 && word.starts_with('-') {
            state.short_cluster(&word[1..], &mut it)?;
        } else {
            state.set_url(word);
        }
    }
    state.finish()
}

// ---------------------------------------------------------------------------------------------
// Tokenizer: POSIX-shell-style word splitting
// ---------------------------------------------------------------------------------------------

fn read_hex(chars: &[char], i: &mut usize, max: usize) -> Option<u32> {
    let mut value = 0u32;
    let mut digits = 0;
    while digits < max {
        match chars.get(*i).and_then(|c| c.to_digit(16)) {
            Some(d) => {
                value = value * 16 + d;
                *i += 1;
                digits += 1;
            }
            None => break,
        }
    }
    (digits > 0).then_some(value)
}

fn push_char(out: &mut Vec<u8>, c: char) {
    let mut buf = [0u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
}

/// Handles one `\x` escape inside `$'...'`. Output is raw bytes, because `\xc3\xa9` means the two
/// bytes of "é", not two Latin-1 characters; the caller decodes the whole quote as UTF-8.
fn ansi_c_escape(chars: &[char], i: &mut usize, out: &mut Vec<u8>) -> Result<(), String> {
    // `*i` is just past the backslash.
    let Some(&e) = chars.get(*i) else {
        return Err("unterminated $'...' quote".into());
    };
    *i += 1;
    match e {
        'n' => out.push(b'\n'),
        't' => out.push(b'\t'),
        'r' => out.push(b'\r'),
        'a' => out.push(0x07),
        'b' => out.push(0x08),
        'e' | 'E' => out.push(0x1b),
        'f' => out.push(0x0c),
        'v' => out.push(0x0b),
        '\\' | '\'' | '"' | '?' => push_char(out, e),
        'x' => match read_hex(chars, i, 2) {
            Some(v) => out.push(v as u8),
            None => out.extend_from_slice(b"\\x"),
        },
        'u' | 'U' => match read_hex(chars, i, if e == 'u' { 4 } else { 8 }) {
            Some(v) => push_char(out, char::from_u32(v).unwrap_or('\u{fffd}')),
            None => {
                out.push(b'\\');
                push_char(out, e);
            }
        },
        '0'..='7' => {
            let mut v = e.to_digit(8).unwrap_or(0);
            let mut n = 1;
            while n < 3 {
                match chars.get(*i).and_then(|c| c.to_digit(8)) {
                    Some(d) => {
                        v = v * 8 + d;
                        *i += 1;
                        n += 1;
                    }
                    None => break,
                }
            }
            out.push(v as u8);
        }
        other => {
            out.push(b'\\');
            push_char(out, other);
        }
    }
    Ok(())
}

/// What the tokenizer found besides the words themselves.
#[derive(Default)]
struct Tokens {
    words: Vec<String>,
    /// `$NAME` / `${NAME}` references, turned into `{{NAME}}` placeholders in the words.
    shell_vars: Vec<String>,
    /// `$(...)` or backticks were present; they are left as text, never evaluated.
    substitution: bool,
    /// An unquoted `|`, `&&`, `||` or `;` ended the command; this is the operator.
    stopped_at: Option<&'static str>,
    /// A second `curl ...` command follows on a later line; only the first is imported.
    second_command: bool,
    /// `>`/`<` redirections were skipped.
    redirected: bool,
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

/// Handles a `$` outside single quotes. Returns true if it consumed input.
fn expand_dollar(chars: &[char], i: &mut usize, cur: &mut String, t: &mut Tokens) -> bool {
    let name = match chars.get(*i + 1) {
        Some('{') => {
            let end = chars[*i + 2..].iter().position(|&c| c == '}').map(|e| *i + 2 + e);
            match end {
                Some(end) => {
                    let name: String = chars[*i + 2..end].iter().collect();
                    if name.chars().next().is_some_and(is_ident_start)
                        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    {
                        *i = end + 1;
                        Some(name)
                    } else {
                        None
                    }
                }
                None => None,
            }
        }
        Some('(') => {
            t.substitution = true;
            None
        }
        Some(&c) if is_ident_start(c) => {
            let mut j = *i + 1;
            let mut name = String::new();
            while let Some(&c) = chars.get(j).filter(|c| c.is_ascii_alphanumeric() || **c == '_') {
                name.push(c);
                j += 1;
            }
            *i = j;
            Some(name)
        }
        _ => None,
    };
    match name {
        Some(name) => {
            cur.push_str(&format!("{{{{{name}}}}}"));
            if !t.shell_vars.contains(&name) {
                t.shell_vars.push(name);
            }
            true
        }
        None => false,
    }
}

/// Whether the text from `from` (after optional blank space) starts another `curl` command.
fn next_line_starts_curl(chars: &[char], from: usize) -> bool {
    let rest: String = chars[from..].iter().take(16).collect();
    let rest = rest.trim_start();
    rest.strip_prefix("curl").is_some_and(|r| {
        let r = r.strip_prefix(".exe").unwrap_or(r);
        r.starts_with(char::is_whitespace)
    })
}

fn tokenize(text: &str) -> Result<Tokens, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut t = Tokens::default();
    let mut cur = String::new();
    let mut in_word = false;
    // Set after a redirection operator: the next word is its target and is dropped.
    let mut skip_word = false;
    let mut i = 0;

    // Ends the word being built (dropping it if it was a redirection target).
    macro_rules! end_word {
        () => {
            if in_word {
                let word = std::mem::take(&mut cur);
                if skip_word {
                    skip_word = false;
                } else {
                    t.words.push(word);
                }
                in_word = false;
            }
        };
    }

    while i < chars.len() {
        let c = chars[i];
        match c {
            '\n' if t.words.len() + usize::from(in_word) > 0 && next_line_starts_curl(&chars, i + 1) => {
                t.second_command = true;
                break;
            }
            c if c.is_whitespace() => {
                end_word!();
                i += 1;
            }
            '\\' => match chars.get(i + 1) {
                Some('\n') => i += 2, // line continuation
                Some('\r') if chars.get(i + 2) == Some(&'\n') => i += 3,
                Some(&next) => {
                    cur.push(next);
                    in_word = true;
                    i += 2;
                }
                None => {
                    cur.push('\\');
                    in_word = true;
                    i += 1;
                }
            },
            '\'' => {
                in_word = true;
                i += 1;
                loop {
                    match chars.get(i) {
                        None => return Err("unterminated single quote".into()),
                        Some('\'') => {
                            i += 1;
                            break;
                        }
                        Some(&ch) => {
                            cur.push(ch);
                            i += 1;
                        }
                    }
                }
            }
            '"' => {
                in_word = true;
                i += 1;
                loop {
                    match chars.get(i) {
                        None => return Err("unterminated double quote".into()),
                        Some('"') => {
                            i += 1;
                            break;
                        }
                        Some('\\') => match chars.get(i + 1) {
                            Some(&n) if matches!(n, '"' | '\\' | '$' | '`') => {
                                cur.push(n);
                                i += 2;
                            }
                            Some('\n') => i += 2,
                            _ => {
                                cur.push('\\');
                                i += 1;
                            }
                        },
                        Some('$') => {
                            if !expand_dollar(&chars, &mut i, &mut cur, &mut t) {
                                cur.push('$');
                                i += 1;
                            }
                        }
                        Some('`') => {
                            t.substitution = true;
                            cur.push('`');
                            i += 1;
                        }
                        Some(&ch) => {
                            cur.push(ch);
                            i += 1;
                        }
                    }
                }
            }
            '$' if chars.get(i + 1) == Some(&'\'') => {
                in_word = true;
                i += 2;
                let mut raw: Vec<u8> = Vec::new();
                loop {
                    match chars.get(i) {
                        None => return Err("unterminated $'...' quote".into()),
                        Some('\'') => {
                            i += 1;
                            break;
                        }
                        Some('\\') => {
                            i += 1;
                            ansi_c_escape(&chars, &mut i, &mut raw)?;
                        }
                        Some(&ch) => {
                            push_char(&mut raw, ch);
                            i += 1;
                        }
                    }
                }
                cur.push_str(&String::from_utf8_lossy(&raw));
            }
            '$' => {
                in_word = true;
                if !expand_dollar(&chars, &mut i, &mut cur, &mut t) {
                    cur.push('$');
                    i += 1;
                }
            }
            '`' => {
                in_word = true;
                t.substitution = true;
                cur.push('`');
                i += 1;
            }
            '#' if !in_word => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            // Unquoted control operators end the command: whatever follows isn't part of it.
            '|' => {
                t.stopped_at = Some(if chars.get(i + 1) == Some(&'|') { "||" } else { "|" });
                break;
            }
            '&' if chars.get(i + 1) == Some(&'&') => {
                t.stopped_at = Some("&&");
                break;
            }
            ';' if chars.get(i + 1).is_none_or(|n| n.is_whitespace()) => {
                t.stopped_at = Some(";");
                break;
            }
            '>' | '<' => {
                // A leading file descriptor ("2>") belongs to the operator, not to a word.
                if in_word && cur.chars().all(|d| d.is_ascii_digit()) {
                    cur.clear();
                    in_word = false;
                } else {
                    end_word!();
                }
                i += 1;
                if chars.get(i) == Some(&c) {
                    i += 1; // >> or <<
                }
                if chars.get(i) == Some(&'&') {
                    // 2>&1: a descriptor, not a file; there is no target word.
                    i += 1;
                    while chars.get(i).is_some_and(|d| d.is_ascii_digit() || *d == '-') {
                        i += 1;
                    }
                } else {
                    skip_word = true;
                }
                t.redirected = true;
            }
            _ => {
                cur.push(c);
                in_word = true;
                i += 1;
            }
        }
    }
    if in_word && !skip_word {
        t.words.push(cur);
    }
    Ok(t)
}

#[cfg(test)]
fn split_words(text: &str) -> Result<Vec<String>, String> {
    tokenize(text).map(|t| t.words)
}

// ---------------------------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Kind {
    Method,
    Header,
    Data,
    DataRaw,
    DataUrlencode,
    User,
    UserAgent,
    Referer,
    Cookie,
    Url,
    Get,
    Head,
    OAuth2Bearer,
    /// `--json`: data plus JSON Content-Type and Accept headers.
    Json,
    /// Accepted and ignored without comment (does not change what is sent).
    Ignore,
    /// Accepted, ignored, and the user is told why.
    Warn(&'static str),
    /// Like `Warn`, but the option also implies a method in curl (-F means POST, -T means PUT).
    Implies(Method, &'static str),
}

struct Flag {
    long: &'static str,
    short: Option<char>,
    value: bool,
    kind: Kind,
}

const fn flag(long: &'static str, short: Option<char>, value: bool, kind: Kind) -> Flag {
    Flag {
        long,
        short,
        value,
        kind,
    }
}

const FLAGS: &[Flag] = &[
    flag("request", Some('X'), true, Kind::Method),
    flag("header", Some('H'), true, Kind::Header),
    flag("data", Some('d'), true, Kind::Data),
    flag("data-ascii", None, true, Kind::Data),
    flag("data-binary", None, true, Kind::Data),
    flag("data-raw", None, true, Kind::DataRaw),
    flag("data-urlencode", None, true, Kind::DataUrlencode),
    flag("user", Some('u'), true, Kind::User),
    flag("user-agent", Some('A'), true, Kind::UserAgent),
    flag("referer", Some('e'), true, Kind::Referer),
    flag("cookie", Some('b'), true, Kind::Cookie),
    flag("url", None, true, Kind::Url),
    flag("get", Some('G'), false, Kind::Get),
    flag("head", Some('I'), false, Kind::Head),
    flag("oauth2-bearer", None, true, Kind::OAuth2Bearer),
    flag("json", None, true, Kind::Json),
    flag(
        "form",
        Some('F'),
        true,
        Kind::Implies(Method::Post, "-F multipart forms aren't supported yet"),
    ),
    flag(
        "form-string",
        None,
        true,
        Kind::Implies(Method::Post, "--form-string multipart forms aren't supported yet"),
    ),
    flag(
        "upload-file",
        Some('T'),
        true,
        Kind::Implies(Method::Put, "-T file uploads aren't supported"),
    ),
    flag(
        "insecure",
        Some('k'),
        false,
        Kind::Warn("-k: TLS certificates are always verified"),
    ),
    flag("output", Some('o'), true, Kind::Warn("-o ignored (use \"Save body\")")),
    flag("proxy", Some('x'), true, Kind::Warn("-x proxy ignored")),
    flag(
        "cert",
        Some('E'),
        true,
        Kind::Warn("--cert client certificates aren't supported"),
    ),
    flag(
        "key",
        None,
        true,
        Kind::Warn("--key client certificates aren't supported"),
    ),
    flag("cacert", None, true, Kind::Warn("--cacert ignored")),
    flag("cookie-jar", Some('c'), true, Kind::Warn("-c cookie jar ignored")),
    flag("config", Some('K'), true, Kind::Warn("-K config files aren't read")),
    flag("resolve", None, true, Kind::Warn("--resolve ignored")),
    flag(
        "location-trusted",
        None,
        false,
        Kind::Warn("--location-trusted ignored"),
    ),
    flag("location", Some('L'), false, Kind::Ignore),
    flag("compressed", None, false, Kind::Ignore),
    flag("silent", Some('s'), false, Kind::Ignore),
    flag("show-error", Some('S'), false, Kind::Ignore),
    flag("verbose", Some('v'), false, Kind::Ignore),
    flag("include", Some('i'), false, Kind::Ignore),
    flag("globoff", Some('g'), false, Kind::Ignore),
    flag("fail", Some('f'), false, Kind::Ignore),
    flag("fail-with-body", None, false, Kind::Ignore),
    flag("progress-bar", Some('#'), false, Kind::Ignore),
    flag("http1.0", Some('0'), false, Kind::Ignore),
    flag("disable", Some('q'), false, Kind::Ignore),
    flag("netrc", Some('n'), false, Kind::Ignore),
    flag("remote-name", Some('O'), false, Kind::Ignore),
    flag("remote-header-name", Some('J'), false, Kind::Ignore),
    flag("remote-time", Some('R'), false, Kind::Ignore),
    flag("parallel", Some('Z'), false, Kind::Ignore),
    flag("junk-session-cookies", Some('j'), false, Kind::Ignore),
    flag("use-ascii", Some('B'), false, Kind::Ignore),
    flag("append", Some('a'), false, Kind::Ignore),
    flag("http2-prior-knowledge", None, false, Kind::Ignore),
    flag("retry-connrefused", None, false, Kind::Ignore),
    flag("retry-all-errors", None, false, Kind::Ignore),
    flag("create-dirs", None, false, Kind::Ignore),
    flag("ssl-no-revoke", None, false, Kind::Ignore),
    flag("no-buffer", Some('N'), false, Kind::Ignore),
    flag("ipv4", Some('4'), false, Kind::Ignore),
    flag("ipv6", Some('6'), false, Kind::Ignore),
    flag("http1.1", None, false, Kind::Ignore),
    flag("http2", None, false, Kind::Ignore),
    flag("tcp-nodelay", None, false, Kind::Ignore),
    flag("max-time", Some('m'), true, Kind::Ignore),
    flag("connect-timeout", None, true, Kind::Ignore),
    flag("max-redirs", None, true, Kind::Ignore),
    flag("retry", None, true, Kind::Ignore),
    flag("write-out", Some('w'), true, Kind::Ignore),
    flag("dump-header", Some('D'), true, Kind::Ignore),
    // Value-taking options that don't change the request; listing them keeps their value from
    // being mistaken for the URL.
    flag("retry-delay", None, true, Kind::Ignore),
    flag("retry-max-time", None, true, Kind::Ignore),
    flag("limit-rate", None, true, Kind::Ignore),
    flag("range", Some('r'), true, Kind::Ignore),
    flag("proto", None, true, Kind::Ignore),
    flag("proto-redir", None, true, Kind::Ignore),
    flag("connect-to", None, true, Kind::Ignore),
    flag("interface", None, true, Kind::Ignore),
    flag("dns-servers", None, true, Kind::Ignore),
    flag("local-port", None, true, Kind::Ignore),
    flag("speed-limit", Some('Y'), true, Kind::Ignore),
    flag("speed-time", Some('y'), true, Kind::Ignore),
    flag("keepalive-time", None, true, Kind::Ignore),
    flag("expect100-timeout", None, true, Kind::Ignore),
    flag("max-filesize", None, true, Kind::Ignore),
    flag("trace", None, true, Kind::Ignore),
    flag("trace-ascii", None, true, Kind::Ignore),
    flag("stderr", None, true, Kind::Ignore),
    flag("time-cond", Some('z'), true, Kind::Ignore),
    flag("continue-at", Some('C'), true, Kind::Ignore),
    flag("quote", Some('Q'), true, Kind::Ignore),
    flag("netrc-file", None, true, Kind::Ignore),
    flag("proxy-user", Some('U'), true, Kind::Warn("--proxy-user ignored")),
    flag("cert-type", None, true, Kind::Warn("--cert-type ignored")),
    flag("key-type", None, true, Kind::Warn("--key-type ignored")),
    flag("pass", None, true, Kind::Warn("--pass ignored")),
    flag("ciphers", None, true, Kind::Warn("--ciphers ignored")),
    flag("unix-socket", None, true, Kind::Warn("--unix-socket isn't supported")),
    flag(
        "digest",
        None,
        false,
        Kind::Warn("--digest authentication isn't supported"),
    ),
    flag("ntlm", None, false, Kind::Warn("--ntlm authentication isn't supported")),
    flag(
        "negotiate",
        None,
        false,
        Kind::Warn("--negotiate authentication isn't supported"),
    ),
    flag(
        "anyauth",
        None,
        false,
        Kind::Warn("--anyauth isn't supported; credentials are sent as Basic"),
    ),
    flag("basic", None, false, Kind::Ignore),
    flag("http3", None, false, Kind::Ignore),
    flag("tlsv1.0", None, false, Kind::Ignore),
    flag("tlsv1.1", None, false, Kind::Ignore),
    flag("tlsv1.2", None, false, Kind::Ignore),
    flag("tlsv1.3", None, false, Kind::Ignore),
    flag("no-keepalive", None, false, Kind::Ignore),
    flag("tcp-fastopen", None, false, Kind::Ignore),
    flag("trace-time", None, false, Kind::Ignore),
    flag("raw", None, false, Kind::Ignore),
    flag("path-as-is", None, false, Kind::Ignore),
    flag("no-progress-meter", None, false, Kind::Ignore),
];

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Form rows for `k=v` parts, or `None` if turning them into rows could change the bytes sent
/// (a part without `=`, or percent-escapes that are not valid UTF-8).
fn decode_form(parts: &[&str]) -> Option<Vec<KeyValue>> {
    parts
        .iter()
        .map(|p| {
            let (k, v) = p.split_once('=')?;
            let key = percent_decode_strict(k)?;
            // The send path drops rows with a blank key, so such data must stay raw.
            if key.trim().is_empty() {
                return None;
            }
            Some(KeyValue::new(&key, &percent_decode_strict(v)?))
        })
        .collect()
}

fn percent_decode_strict(s: &str) -> Option<String> {
    String::from_utf8(percent_decode_bytes(s)).ok()
}

#[cfg(test)]
fn percent_decode(s: &str) -> String {
    String::from_utf8_lossy(&percent_decode_bytes(s)).into_owned()
}

/// Standard base64 (RFC 4648, with padding).
fn base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(TABLE[((n >> shift) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn percent_decode_bytes(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() && bytes[i + 1].is_ascii_hexdigit() && bytes[i + 2].is_ascii_hexdigit() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("00");
                out.push(u8::from_str_radix(hex, 16).unwrap_or(b'?'));
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

#[derive(Default)]
struct State {
    method: Option<String>,
    headers: Vec<(String, String)>,
    /// A `Content-Type:` with no value was given, which removes the header in curl.
    content_type_removed: bool,
    data: Vec<String>,
    user: Option<String>,
    bearer: Option<String>,
    url: Option<String>,
    extra_urls: usize,
    get: bool,
    head: bool,
    /// `--json` was used: add the JSON headers unless the command set its own.
    json: bool,
    /// `-A` / `-e`: used only if the command has no explicit `-H` for the same header.
    user_agent: Option<String>,
    referer: Option<String>,
    /// Some `-d`-style option was seen (even with empty data or an `@file`): curl would POST.
    saw_data: bool,
    /// Method implied by an option we otherwise cannot honour (-F, -T).
    implied_method: Option<Method>,
    warnings: Vec<String>,
}

impl State {
    fn warn(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        if !self.warnings.contains(&msg) {
            self.warnings.push(msg);
        }
    }

    fn set_url(&mut self, url: String) {
        if self.url.is_none() {
            self.url = Some(url);
        } else {
            self.extra_urls += 1;
        }
    }

    fn long_option(&mut self, spec: &str, it: &mut Words) -> Result<(), String> {
        let (name, inline) = match spec.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (spec, None),
        };
        match FLAGS.iter().find(|f| f.long == name) {
            Some(f) => {
                let value = if f.value {
                    Some(
                        inline
                            .or_else(|| it.next())
                            .ok_or_else(|| format!("--{name} needs a value"))?,
                    )
                } else {
                    None
                };
                self.apply(f.kind, value);
            }
            None if inline.is_some() => self.warn(format!("--{name} ignored (unknown option)")),
            None => self.skip_unknown(&format!("--{name}"), it),
        }
        Ok(())
    }

    /// An option we don't know: guess whether the next word is its value (e.g. `--retry-delay 2`)
    /// so that it is not mistaken for the URL. A word that looks like a URL is never swallowed.
    fn skip_unknown(&mut self, name: &str, it: &mut Words) {
        // Swallow the next word as the option's value only if it isn't a URL and a URL is still
        // to be found (already seen, or coming later); otherwise it is probably a bare host name.
        let later_url = it.clone().skip(1).any(|w| looks_like_url(&w));
        let takes_next = it
            .peek()
            .is_some_and(|next| !next.starts_with('-') && !looks_like_url(next) && (self.url.is_some() || later_url));
        if takes_next {
            it.next();
            self.warn(format!("{name} ignored (unknown option, and the value after it)"));
        } else {
            self.warn(format!("{name} ignored (unknown option)"));
        }
    }

    fn short_cluster(&mut self, cluster: &str, it: &mut Words) -> Result<(), String> {
        for (pos, c) in cluster.char_indices() {
            match FLAGS.iter().find(|f| f.short == Some(c)) {
                Some(f) if f.value => {
                    let rest = &cluster[pos + c.len_utf8()..];
                    let value = if rest.is_empty() {
                        it.next().ok_or_else(|| format!("-{c} needs a value"))?
                    } else {
                        rest.to_string()
                    };
                    self.apply(f.kind, Some(value));
                    return Ok(());
                }
                Some(f) => self.apply(f.kind, None),
                None if pos + c.len_utf8() == cluster.len() => self.skip_unknown(&format!("-{c}"), it),
                None => self.warn(format!("-{c} ignored (unknown option)")),
            }
        }
        Ok(())
    }

    fn apply(&mut self, kind: Kind, value: Option<String>) {
        let v = value.unwrap_or_default();
        match kind {
            Kind::Method => self.method = Some(v.to_uppercase()),
            Kind::Header => self.add_header(&v),
            Kind::Data => {
                self.saw_data = true;
                if v.starts_with('@') {
                    self.warn("-d @file isn't supported (files are never read)");
                } else {
                    self.data.push(v);
                }
            }
            Kind::DataRaw => {
                self.saw_data = true;
                self.data.push(v);
            }
            Kind::DataUrlencode => {
                self.saw_data = true;
                self.add_urlencoded(&v);
            }
            Kind::User => self.user = Some(v),
            Kind::UserAgent => self.user_agent = Some(v),
            Kind::Referer => self.referer = Some(v),
            Kind::Cookie => {
                if v.contains('=') {
                    self.headers.push(("Cookie".into(), v));
                } else {
                    self.warn("-b with a cookie file isn't supported");
                }
            }
            Kind::Url => self.set_url(v),
            Kind::Get => self.get = true,
            Kind::Head => self.head = true,
            Kind::OAuth2Bearer => self.bearer = Some(v),
            Kind::Json => {
                self.saw_data = true;
                if v.starts_with('@') {
                    self.warn("--json @file isn't supported (files are never read)");
                } else {
                    self.data.push(v);
                    self.json = true;
                }
            }
            Kind::Ignore => {}
            Kind::Warn(msg) => self.warn(msg),
            Kind::Implies(method, msg) => {
                self.implied_method.get_or_insert(method);
                self.warn(msg);
            }
        }
    }

    fn add_header(&mut self, raw: &str) {
        if let Some((name, value)) = raw.split_once(':') {
            let (name, value) = (name.trim(), value.trim());
            if name.is_empty() {
                self.warn(format!("header {raw:?} ignored (no name)"));
            } else if value.is_empty() {
                // `Name:` removes a header in curl; there is nothing to send.
                if name.eq_ignore_ascii_case("content-type") {
                    self.content_type_removed = true;
                }
            } else {
                self.headers.push((name.to_string(), value.to_string()));
            }
        } else if let Some(name) = raw.trim().strip_suffix(';') {
            // `Name;` is curl's syntax for sending an empty header.
            self.headers.push((name.trim().to_string(), String::new()));
        } else {
            self.warn(format!("header {raw:?} ignored (expected \"Name: value\")"));
        }
    }

    fn add_urlencoded(&mut self, v: &str) {
        if v.starts_with('@') {
            self.warn("--data-urlencode @file isn't supported (files are never read)");
        } else if let Some(content) = v.strip_prefix('=') {
            self.data.push(percent_encode(content));
        } else if let Some((name, content)) = v.split_once('=') {
            self.data.push(format!("{name}={}", percent_encode(content)));
        } else if v.contains('@') {
            self.warn("--data-urlencode name@file isn't supported (files are never read)");
        } else {
            self.data.push(percent_encode(v));
        }
    }

    fn finish(mut self) -> Result<Imported, String> {
        let mut url = self.url.take().ok_or("no URL found in the curl command")?;
        if self.extra_urls > 0 {
            self.warn(format!(
                "{} extra URL(s) ignored; only the first is used",
                self.extra_urls
            ));
        }
        let data = self.data.join("&");
        if self.get && !data.is_empty() {
            // The query goes before any #fragment, or it would never be sent.
            let fragment = url.find('#').map(|at| url.split_off(at)).unwrap_or_default();
            url.push(if url.contains('?') { '&' } else { '?' });
            url.push_str(&data);
            url.push_str(&fragment);
        }
        let has_body = !self.get && !data.is_empty();

        let method = match self.method.take().as_deref() {
            Some("GET") => Method::Get,
            Some("POST") => Method::Post,
            Some("PUT") => Method::Put,
            Some("PATCH") => Method::Patch,
            Some("DELETE") => Method::Delete,
            Some("HEAD") => Method::Head,
            Some("OPTIONS") => Method::Options,
            Some(other) => {
                self.warn(format!("method {other} isn't supported; using GET"));
                Method::Get
            }
            None if self.head => Method::Head,
            None if self.get => Method::Get,
            None if self.implied_method.is_some() => self.implied_method.unwrap_or(Method::Get),
            None if has_body || self.saw_data => Method::Post,
            None => Method::Get,
        };

        // Headers that postrust manages itself or that the Auth tab covers.
        let mut rows: Vec<(String, String)> = Vec::new();
        let mut bearer = self.bearer.take();
        for (name, value) in std::mem::take(&mut self.headers) {
            if name.eq_ignore_ascii_case("content-length") {
                continue;
            } else if name.eq_ignore_ascii_case("accept-encoding") {
                self.warn("Accept-Encoding was dropped (postrust negotiates compression itself)");
            } else if name.eq_ignore_ascii_case("authorization")
                && bearer.is_none()
                && value.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("bearer "))
            {
                bearer = Some(value[7..].trim().to_string());
            } else {
                rows.push((name, value));
            }
        }

        // -A / -e apply only when no explicit -H sets the same header (the -H wins in curl).
        for (name, value) in [("User-Agent", self.user_agent.take()), ("Referer", self.referer.take())] {
            if let Some(value) = value
                && !rows.iter().any(|(n, _)| n.eq_ignore_ascii_case(name))
            {
                rows.push((name.to_string(), value));
            }
        }
        // curl joins several -b options into one Cookie header.
        let cookies: Vec<String> = rows
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case("cookie"))
            .map(|(_, v)| v.clone())
            .collect();
        if cookies.len() > 1 {
            rows.retain(|(n, _)| !n.eq_ignore_ascii_case("cookie"));
            rows.push(("Cookie".into(), cookies.join("; ")));
        }

        // In curl an explicit Authorization header (or --oauth2-bearer) overrides -u.
        let mut user = self.user.take();
        if user.is_some() && rows.iter().any(|(n, _)| n.eq_ignore_ascii_case("authorization")) {
            self.warn("-u ignored because an Authorization header is set (curl uses the header)");
            user = None;
        }
        let auth = match (bearer, user) {
            (Some(token), _) => Auth::Bearer { token },
            (None, Some(user)) => {
                let (username, password) = match user.split_once(':') {
                    Some((u, p)) => (u.to_string(), p.to_string()),
                    None => (user.clone(), String::new()),
                };
                if username.is_empty() {
                    // The Auth tab treats an empty username as "not filled in" and sends nothing,
                    // so keep the credentials as an explicit header, which is what curl sends.
                    rows.push(("Authorization".into(), format!("Basic {}", base64(user.as_bytes()))));
                    self.warn("-u with an empty username was imported as an Authorization header");
                    Auth::None
                } else {
                    Auth::Basic { username, password }
                }
            }
            (None, None) => Auth::None,
        };

        let mut content_type = rows
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.to_lowercase())
            .or_else(|| self.content_type_removed.then(String::new));

        // --json implies JSON Content-Type and Accept headers unless the command set its own.
        if self.json {
            if content_type.is_none() {
                rows.push(("Content-Type".into(), "application/json".into()));
                content_type = Some("application/json".into());
            }
            if !rows.iter().any(|(n, _)| n.eq_ignore_ascii_case("accept")) {
                rows.push(("Accept".into(), "application/json".into()));
            }
        }

        let (mut body_kind, mut body, mut form) = (BodyKind::None, String::new(), Vec::new());
        if has_body {
            let json_like = matches!(data.trim_start().as_bytes().first(), Some(b'{' | b'['));
            // Every `&` separated part is kept, empty ones included, so nothing is lost on the wire.
            let parts: Vec<&str> = data.split('&').collect();
            let form_like = parts
                .iter()
                .all(|p| p.contains('=') && !p.contains(char::is_whitespace) && !p.contains(['{', '}']));
            let bare_form_type = content_type
                .as_deref()
                .is_some_and(|c| c.trim() == "application/x-www-form-urlencoded");
            // curl labels `-d` data as a form unless told otherwise, so key=value data without a
            // Content-Type is a form too. JSON-looking data stays raw even then. A form type with
            // parameters (`; charset=...`) stays raw as well, so the header and bytes are kept.
            let decoded = if !json_like && (bare_form_type || (content_type.is_none() && form_like)) {
                decode_form(&parts)
            } else {
                None
            };
            if let Some(rows_form) = decoded {
                body_kind = BodyKind::Form;
                form = rows_form;
                if bare_form_type {
                    rows.retain(|(n, _)| !n.eq_ignore_ascii_case("content-type"));
                }
            } else {
                body_kind = BodyKind::Raw;
                body = data;
                if content_type.is_none() {
                    // curl labels `-d` data as a form even when it isn't one, while postrust's
                    // Raw body sends no Content-Type; spell it out so the request stays identical.
                    rows.push(("Content-Type".into(), "application/x-www-form-urlencoded".into()));
                    self.warn(
                        "No Content-Type was given, so curl would send application/x-www-form-urlencoded; \
                         that header was added",
                    );
                }
            }
        }

        let request = Request {
            name: String::new(),
            method,
            url,
            params: Vec::new(),
            headers: rows.into_iter().map(|(k, v)| KeyValue::new(&k, &v)).collect(),
            body_kind,
            body,
            form,
            auth,
        };
        Ok(Imported {
            request,
            warnings: self.warnings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(cmd: &str) -> Imported {
        parse(cmd).unwrap_or_else(|e| panic!("{cmd:?} failed: {e}"))
    }

    fn header<'a>(r: &'a Request, name: &str) -> Option<&'a str> {
        r.headers
            .iter()
            .find(|h| h.key.eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str())
    }

    #[test]
    fn detects_curl_commands_only() {
        assert!(looks_like_curl("curl https://a.test"));
        assert!(looks_like_curl("  \n curl -X POST x"));
        assert!(looks_like_curl("curl.exe https://a.test"));
        assert!(looks_like_curl("curl\n  -H 'x: y' url"));
        assert!(!looks_like_curl("curl"));
        assert!(!looks_like_curl("curling https://a.test"));
        assert!(!looks_like_curl("https://a.test/curl https://x"));
        assert!(!looks_like_curl("echo curl https://a.test"));
        assert!(!looks_like_curl(""));
    }

    #[test]
    fn simple_get() {
        let i = ok("curl https://api.test/users?id=1");
        assert_eq!(i.request.method, Method::Get);
        assert_eq!(i.request.url, "https://api.test/users?id=1");
        assert_eq!(i.request.body_kind, BodyKind::None);
        assert!(i.warnings.is_empty());
    }

    #[test]
    fn post_with_json_body_and_headers() {
        let i = ok(
            r#"curl -X POST 'https://api.test/users' -H 'Content-Type: application/json' -H "X-Id: 7" -d '{"name":"ada"}'"#,
        );
        let r = i.request;
        assert_eq!(r.method, Method::Post);
        assert_eq!(r.body_kind, BodyKind::Raw);
        assert_eq!(r.body, r#"{"name":"ada"}"#);
        assert_eq!(header(&r, "content-type"), Some("application/json"));
        assert_eq!(header(&r, "x-id"), Some("7"));
    }

    #[test]
    fn data_implies_post_and_get_flag_moves_data_to_the_query() {
        assert_eq!(ok("curl https://a.test -d 'x=1'").request.method, Method::Post);
        let g = ok("curl -G https://a.test/s -d q=rust -d page=2").request;
        assert_eq!(g.method, Method::Get);
        assert_eq!(g.url, "https://a.test/s?q=rust&page=2");
        assert_eq!(g.body_kind, BodyKind::None);
        let g2 = ok("curl -G 'https://a.test/s?a=1' --data-urlencode 'q=a b'").request;
        assert_eq!(g2.url, "https://a.test/s?a=1&q=a%20b");
        assert_eq!(ok("curl -I https://a.test").request.method, Method::Head);
        assert_eq!(ok("curl --head https://a.test").request.method, Method::Head);
        assert_eq!(ok("curl -X GET https://a.test -d x").request.method, Method::Get);
    }

    #[test]
    fn chrome_style_copy_as_curl() {
        let cmd = "curl 'https://api.test/v1/items?limit=5' \\\n  -H 'accept: application/json, text/plain, */*' \\\n  -H 'accept-encoding: gzip, deflate, br, zstd' \\\n  -H 'authorization: Bearer abc.def' \\\n  -H 'content-type: application/json' \\\n  -b 'sid=1; theme=dark' \\\n  --data-raw $'{\"q\":\"it\\'s\\\\nline\",\"e\":\"\\u00e9\"}' \\\n  --compressed";
        let i = ok(cmd);
        let r = &i.request;
        assert_eq!(r.method, Method::Post);
        assert_eq!(r.url, "https://api.test/v1/items?limit=5");
        assert_eq!(
            r.auth,
            Auth::Bearer {
                token: "abc.def".into()
            }
        );
        assert!(header(r, "authorization").is_none(), "moved to the Auth tab");
        assert!(header(r, "accept-encoding").is_none());
        assert_eq!(header(r, "cookie"), Some("sid=1; theme=dark"));
        assert_eq!(r.body, "{\"q\":\"it's\\nline\",\"e\":\"é\"}");
        assert!(
            i.warnings.iter().any(|w| w.contains("Accept-Encoding")),
            "{:?}",
            i.warnings
        );
    }

    #[test]
    fn auth_forms() {
        let r = ok("curl -u alice:s3cret:x https://a.test").request;
        assert_eq!(
            r.auth,
            Auth::Basic {
                username: "alice".into(),
                password: "s3cret:x".into()
            }
        );
        let r = ok("curl --user bob https://a.test").request;
        assert_eq!(
            r.auth,
            Auth::Basic {
                username: "bob".into(),
                password: String::new()
            }
        );
        let r = ok("curl --oauth2-bearer tok https://a.test").request;
        assert_eq!(r.auth, Auth::Bearer { token: "tok".into() });
        let r = ok("curl -H 'Authorization: bearer  tok2 ' https://a.test").request;
        assert_eq!(r.auth, Auth::Bearer { token: "tok2".into() });
        let r = ok("curl -H 'Authorization: Basic YTpi' https://a.test").request;
        assert_eq!(r.auth, Auth::None);
        assert_eq!(
            header(&r, "authorization"),
            Some("Basic YTpi"),
            "other schemes stay as headers"
        );
    }

    #[test]
    fn form_bodies_become_form_rows() {
        let r = ok("curl https://a.test -d 'user+name=ada%20l&city=Z%C3%BCrich' -H 'Content-Type: application/x-www-form-urlencoded'").request;
        assert_eq!(r.body_kind, BodyKind::Form);
        assert_eq!(
            r.form,
            vec![KeyValue::new("user name", "ada l"), KeyValue::new("city", "Zürich")]
        );
        assert!(header(&r, "content-type").is_none(), "postrust sets it for forms");

        // No Content-Type: curl would send this as a form, so it is one.
        let r = ok("curl https://a.test -d a=1 -d b=2").request;
        assert_eq!(r.body_kind, BodyKind::Form);
        assert_eq!(r.form, vec![KeyValue::new("a", "1"), KeyValue::new("b", "2")]);

        // JSON-looking data stays raw even without a header.
        let r = ok(r#"curl https://a.test -d '{"a":"b=c"}'"#).request;
        assert_eq!(r.body_kind, BodyKind::Raw);

        // Plain text, or an explicit non-form type, stays raw.
        assert_eq!(
            ok("curl https://a.test -d 'hello world'").request.body_kind,
            BodyKind::Raw
        );
        let r = ok("curl https://a.test -d 'a=b' -H 'Content-Type: text/plain'").request;
        assert_eq!(r.body_kind, BodyKind::Raw);
        assert_eq!(header(&r, "content-type"), Some("text/plain"));
    }

    #[test]
    fn data_urlencode_forms() {
        let r = ok(
            "curl https://a.test --data-urlencode 'name=a b&c' --data-urlencode '=x y' --data-urlencode 'plain text'",
        )
        .request;
        assert_eq!(r.body_kind, BodyKind::Raw);
        assert_eq!(r.body, "name=a%20b%26c&x%20y&plain%20text");
    }

    #[test]
    fn short_option_clusters_and_attached_values() {
        let r = ok("curl -sSLXPUT -H'X-A: 1' --header=X-B:2 --request=PATCH https://a.test").request;
        assert_eq!(r.method, Method::Patch, "last -X wins");
        assert_eq!(header(&r, "x-a"), Some("1"));
        assert_eq!(header(&r, "x-b"), Some("2"));
        let r = ok("curl -XDELETE https://a.test/1").request;
        assert_eq!(r.method, Method::Delete);
        let r = ok("curl --url https://a.test/x -A 'my-agent/1' -e https://ref.test").request;
        assert_eq!(r.url, "https://a.test/x");
        assert_eq!(header(&r, "user-agent"), Some("my-agent/1"));
        assert_eq!(header(&r, "referer"), Some("https://ref.test"));
    }

    #[test]
    fn unsupported_things_are_reported_not_guessed_and_files_are_never_read() {
        let i = ok("curl https://a.test -F 'f=@/etc/passwd' -d @/etc/shadow -o out.bin -k --cert c.pem --weird-flag");
        let w = i.warnings.join(" | ");
        for needle in ["-F", "-d @file", "-o ignored", "-k", "--cert", "--weird-flag"] {
            assert!(w.contains(needle), "{needle} missing in {w}");
        }
        assert_eq!(i.request.body_kind, BodyKind::None, "nothing was read from the files");
        let i = ok("curl -X PROPFIND https://a.test");
        assert_eq!(i.request.method, Method::Get);
        assert!(i.warnings[0].contains("PROPFIND"));
        let i = ok("curl https://a.test https://b.test");
        assert_eq!(i.request.url, "https://a.test");
        assert!(i.warnings[0].contains("extra URL"));
    }

    #[test]
    fn header_edge_cases() {
        let r = ok("curl https://a.test -H 'X-Empty;' -H 'Content-Length: 99' -H 'Bad header' -H ': nameless'").request;
        assert_eq!(header(&r, "x-empty"), Some(""));
        assert!(header(&r, "content-length").is_none());
        let r = ok("curl https://a.test -d 'a=b' -H 'Content-Type:'").request;
        assert_eq!(
            r.body_kind,
            BodyKind::Raw,
            "an explicit empty Content-Type is not a form"
        );
    }

    #[test]
    fn tokenizer_quoting_rules() {
        let w = split_words(r#"a 'b c' "d \"e\" \$f" g\ h 'it'"'"'s' ''"#).unwrap();
        assert_eq!(w, vec!["a", "b c", "d \"e\" $f", "g h", "it's", ""]);
        assert_eq!(
            split_words("a'b'\"c\"d").unwrap(),
            vec!["abcd"],
            "adjacent pieces concatenate"
        );
        assert_eq!(
            split_words("one \\\n  two").unwrap(),
            vec!["one", "two"],
            "line continuation"
        );
        assert_eq!(split_words("one # comment\ntwo").unwrap(), vec!["one", "two"]);
        assert_eq!(split_words("x#y").unwrap(), vec!["x#y"], "# inside a word is literal");
        assert_eq!(
            split_words(r#""a\qb""#).unwrap(),
            vec![r"a\qb"],
            "unknown escapes keep the backslash"
        );
        assert_eq!(split_words("$'\\x41\\101\\u00e9\\t'").unwrap(), vec!["AA\u{e9}\t"]);
        assert_eq!(split_words("$'\\n\\\\\\''").unwrap(), vec!["\n\\'"]);
    }

    #[test]
    fn errors_are_clear() {
        assert!(parse("echo hi").unwrap_err().contains("not a curl command"));
        assert!(parse("curl").unwrap_err().contains("no URL"));
        assert!(parse("curl -X POST").unwrap_err().contains("no URL"));
        assert!(parse("curl 'https://a.test").unwrap_err().contains("unterminated"));
        assert!(parse("curl \"https://a.test").unwrap_err().contains("unterminated"));
        assert!(parse("curl $'abc").unwrap_err().contains("unterminated"));
        assert!(parse("curl https://a.test -H").unwrap_err().contains("needs a value"));
        assert!(
            parse(&format!("curl https://a.test -d '{}'", "x".repeat(MAX_LEN)))
                .unwrap_err()
                .contains("too large")
        );
    }

    #[test]
    fn odd_input_does_not_panic() {
        for text in [
            "curl -",
            "curl --",
            "curl -- https://a.test",
            "curl --=x https://a.test",
            "curl -é https://a.test",
            "curl https://a.test -d",
            "curl \\",
            "curl $'\\",
            "curl $'\\u",
            "curl $'\\x'",
            "curl '%'",
            "curl https://a.test -d '%zz%4'",
        ] {
            let _ = parse(text); // must not panic; Ok or Err are both fine
        }
        assert_eq!(ok("curl -- https://a.test/-x").request.url, "https://a.test/-x");
    }

    #[test]
    fn multibyte_text_in_an_authorization_header_does_not_panic() {
        // Byte 7 of "Basic ü" falls inside the 2-byte character: slicing there used to panic.
        for value in ["Basic ü", "Béarer x", "ü", "日本語日本語日本語", "😀😀😀"] {
            let i = ok(&format!("curl https://a.test -H 'Authorization: {value}'"));
            assert!(
                i.request
                    .headers
                    .iter()
                    .any(|h| h.key == "Authorization" && h.value == value),
                "{value:?} should stay a plain header: {:?}",
                i.request
            );
        }
        assert_eq!(
            ok("curl https://a.test -H 'Authorization: Bearer é-tok'").request.auth,
            Auth::Bearer { token: "é-tok".into() }
        );
    }

    #[test]
    fn unknown_options_do_not_turn_their_value_into_the_url() {
        for cmd in [
            "curl --retry-delay 2 https://api.test/x",
            "curl --proxy-user u:p https://api.test/x",
            "curl --limit-rate 1.5m https://api.test/x",
            "curl -r 0-99 https://api.test/x",
            "curl -U user:pass https://api.test/x",
            "curl --proto =https https://api.test/x",
            "curl --made-up-option 5 https://api.test/x",
            "curl -W 7 https://api.test/x",
            "curl https://api.test/x --made-up-option 5",
            "curl --connect-to ::host:443 https://api.test/x",
        ] {
            assert_eq!(ok(cmd).request.url, "https://api.test/x", "{cmd}");
        }
        // A word that looks like a URL is never swallowed as an unknown option's value.
        for url in [
            "https://api.test/x",
            "api.test/x",
            "localhost:3000/x",
            "127.0.0.1:8080/x",
        ] {
            let i = ok(&format!("curl --made-up-option {url}"));
            assert_eq!(i.request.url, url);
        }
        // `--name=value` is self-contained.
        assert_eq!(
            ok("curl --made-up=5 https://api.test/x").request.url,
            "https://api.test/x"
        );
    }

    #[test]
    fn json_option_sets_body_and_headers() {
        let r = ok(r#"curl --json '{"a":1}' https://api.test/x"#).request;
        assert_eq!(r.method, Method::Post);
        assert_eq!(r.body_kind, BodyKind::Raw);
        assert_eq!(r.body, r#"{"a":1}"#);
        assert_eq!(header(&r, "content-type"), Some("application/json"));
        assert_eq!(header(&r, "accept"), Some("application/json"));

        // The command's own headers win.
        let r = ok(r#"curl --json '{"a":1}' -H 'Accept: text/csv' -H 'Content-Type: application/vnd.api+json' https://api.test/x"#).request;
        assert_eq!(header(&r, "accept"), Some("text/csv"));
        assert_eq!(header(&r, "content-type"), Some("application/vnd.api+json"));
        assert_eq!(
            r.headers
                .iter()
                .filter(|h| h.key.eq_ignore_ascii_case("accept"))
                .count(),
            1
        );

        let i = ok("curl --json @body.json https://api.test/x");
        assert_eq!(i.request.body_kind, BodyKind::None);
        assert!(
            i.warnings.iter().any(|w| w.contains("--json @file")),
            "{:?}",
            i.warnings
        );
    }

    #[test]
    fn data_without_a_content_type_keeps_curls_form_label() {
        for body in ["hello world", r#"{"a":1}"#, "[1,2]"] {
            let i = ok(&format!("curl https://a.test -d '{body}'"));
            assert_eq!(i.request.body_kind, BodyKind::Raw, "{body}");
            assert_eq!(
                header(&i.request, "content-type"),
                Some("application/x-www-form-urlencoded"),
                "curl would have sent this label, so the request must too: {body}"
            );
            assert!(
                i.warnings.iter().any(|w| w.contains("No Content-Type")),
                "{:?}",
                i.warnings
            );
        }
        // No extra header when the command chose one, removed it, or the data is a real form.
        let r = ok("curl https://a.test -d 'x y' -H 'Content-Type: text/plain'").request;
        assert_eq!(
            r.headers
                .iter()
                .filter(|h| h.key.eq_ignore_ascii_case("content-type"))
                .count(),
            1
        );
        let r = ok("curl https://a.test -d 'x y' -H 'Content-Type:'").request;
        assert!(header(&r, "content-type").is_none());
        let i = ok("curl https://a.test -d a=1");
        assert_eq!(i.request.body_kind, BodyKind::Form);
        assert!(header(&i.request, "content-type").is_none());
        assert!(i.warnings.is_empty());
    }

    #[test]
    fn hex_and_octal_escapes_are_bytes_decoded_as_utf8() {
        assert_eq!(split_words("$'\\xc3\\xa9'").unwrap(), vec!["é"]);
        assert_eq!(split_words("$'\\303\\251'").unwrap(), vec!["é"]);
        assert_eq!(split_words("$'\\xe2\\x82\\xac1'").unwrap(), vec!["€1"]);
        assert_eq!(
            split_words("$'a\\xc3b'").unwrap(),
            vec!["a\u{fffd}b"],
            "a broken sequence is replaced, not mangled"
        );
        assert_eq!(split_words("$'\\u00e9\\U0001F600'").unwrap(), vec!["é😀"]);
        assert_eq!(
            ok("curl https://a.test --data-raw $'caf\\xc3\\xa9'").request.body,
            "café"
        );
    }

    #[test]
    fn windows_cmd_style_commands_are_rejected_not_misparsed() {
        let cmd = "curl ^\"https://api.test/x^\" ^\n  -H ^\"accept: */*^\" ^\n  --data-raw ^\"^^{^\"a^\":1}^\"";
        let err = parse(cmd).unwrap_err();
        assert!(err.contains("cmd-style"), "{err}");
        assert!(
            parse("curl https://a.test ^\n  --compressed")
                .unwrap_err()
                .contains("cmd-style")
        );
        // A caret inside a bash command is just a character.
        let i = ok("curl https://a.test -d 'a^\"b' -H 'X: 1'");
        assert_eq!(i.request.body, "a^\"b");
        assert_eq!(
            ok("curl 'https://a.test/^x' -H 'X: ^'").request.url,
            "https://a.test/^x"
        );
    }

    #[test]
    fn get_data_goes_before_the_fragment() {
        assert_eq!(
            ok("curl -G 'https://a.test/p#top' -d q=1").request.url,
            "https://a.test/p?q=1#top"
        );
        assert_eq!(
            ok("curl -G 'https://a.test/p?a=1#top' -d q=1 -d r=2").request.url,
            "https://a.test/p?a=1&q=1&r=2#top"
        );
        assert_eq!(
            ok("curl -G 'https://a.test/p' -d q=1").request.url,
            "https://a.test/p?q=1"
        );
    }

    #[test]
    fn explicit_authorization_beats_dash_u_like_curl() {
        let i = ok("curl -u a:b -H 'Authorization: Bearer tok' https://a.test");
        assert_eq!(i.request.auth, Auth::Bearer { token: "tok".into() });

        let i = ok("curl -u a:b -H 'Authorization: Basic Zm9vOmJhcg==' https://a.test");
        assert_eq!(
            i.request.auth,
            Auth::None,
            "-u must not override the explicit header at send time"
        );
        assert_eq!(header(&i.request, "authorization"), Some("Basic Zm9vOmJhcg=="));
        assert!(i.warnings.iter().any(|w| w.contains("-u ignored")), "{:?}", i.warnings);

        let i = ok("curl -u a:b --oauth2-bearer tok2 https://a.test");
        assert_eq!(i.request.auth, Auth::Bearer { token: "tok2".into() });
    }

    #[test]
    fn randomized_input_never_panics() {
        // Deterministic xorshift so a failure is reproducible.
        let alphabet: Vec<char> =
            "curl -XHdu'\"\\$#^@=:;&?/. \n\t\u{fc}\u{e9}\u{20ac}\u{1F600}0123456789aAzZ%+{}[]!~*,|<>"
                .chars()
                .collect();
        let words = [
            "curl",
            "https://a.test/x",
            "-H",
            "-d",
            "-u",
            "-X",
            "--json",
            "-G",
            "-F",
            "--data-urlencode",
            "$'",
            "--compressed",
            "-b",
            "Authorization: Bearer ",
            "Authorization: Basic \u{fc}",
        ];
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..30_000 {
            let mut text = String::from("curl ");
            for _ in 0..(next() % 12) {
                if next() % 3 == 0 {
                    text.push_str(words[(next() % words.len() as u64) as usize]);
                } else {
                    for _ in 0..(next() % 8) {
                        text.push(alphabet[(next() % alphabet.len() as u64) as usize]);
                    }
                }
                text.push(' ');
            }
            let _ = parse(&text); // Ok or Err are both fine; panicking is not
            let _ = looks_like_curl(&text);
        }
    }

    #[test]
    fn multipart_and_upload_options_still_imply_their_method() {
        let i = ok("curl https://api.test/upload -F 'file=@a.png' -F 'name=x'");
        assert_eq!(i.request.method, Method::Post, "-F means POST in curl");
        assert_eq!(i.request.body_kind, BodyKind::None);
        assert!(i.warnings.iter().any(|w| w.contains("-F")), "{:?}", i.warnings);
        assert_eq!(ok("curl https://api.test/up -T file.bin").request.method, Method::Put);
        assert_eq!(
            ok("curl https://api.test/up --form-string a=b").request.method,
            Method::Post
        );
        // An explicit method still wins.
        assert_eq!(
            ok("curl -X PATCH https://api.test/up -F a=b").request.method,
            Method::Patch
        );
    }

    #[test]
    fn shell_variables_become_environment_placeholders() {
        let i = ok(r#"curl -H "Authorization: Bearer $TOKEN" "https://$HOST/v1/${RESOURCE}s""#);
        assert_eq!(
            i.request.auth,
            Auth::Bearer {
                token: "{{TOKEN}}".into()
            }
        );
        assert_eq!(i.request.url, "https://{{HOST}}/v1/{{RESOURCE}}s");
        for name in ["TOKEN", "HOST", "RESOURCE"] {
            assert!(
                i.warnings.iter().any(|w| w.contains(&format!("${name}"))),
                "{name}: {:?}",
                i.warnings
            );
        }
        // Single quotes, escapes and non-names stay literal.
        let r =
            ok(r#"curl 'https://a.test/$NOT_EXPANDED' -d "cost: \$5 and $1 and $? and $$" -H 'X: ${also literal}'"#)
                .request;
        assert_eq!(r.url, "https://a.test/$NOT_EXPANDED");
        assert_eq!(r.body, "cost: $5 and $1 and $? and $$");
        assert_eq!(header(&r, "x"), Some("${also literal}"));
        // `${...}` that is not a plain name is left alone.
        assert_eq!(
            ok(r#"curl "https://a.test/${A-b}""#).request.url,
            "https://a.test/${A-b}"
        );
    }

    #[test]
    fn command_substitution_is_flagged_never_evaluated() {
        for cmd in [
            "curl https://a.test/$(whoami)",
            "curl \"https://a.test/`id`\"",
            "curl https://a.test -H \"X: $(date)\"",
        ] {
            let i = ok(cmd);
            assert!(
                i.warnings.iter().any(|w| w.contains("Command substitution")),
                "{cmd}: {:?}",
                i.warnings
            );
        }
        assert!(
            ok("curl 'https://a.test/$(whoami)'").warnings.is_empty(),
            "inside single quotes it is just text"
        );
    }

    #[test]
    fn shell_operators_end_the_command_and_redirections_are_skipped() {
        let i = ok("curl -s https://x.test/a | jq .");
        assert_eq!(i.request.url, "https://x.test/a");
        assert!(i.warnings.iter().any(|w| w.contains("after `|`")), "{:?}", i.warnings);
        for (cmd, op) in [
            ("curl https://x.test/a && echo done", "&&"),
            ("curl https://x.test/a || echo failed", "||"),
            ("curl https://x.test/a ; echo done", ";"),
            ("curl https://x.test/a;", ";"),
        ] {
            let i = ok(cmd);
            assert_eq!(i.request.url, "https://x.test/a", "{cmd}");
            assert!(
                i.warnings.iter().any(|w| w.contains(&format!("`{op}`"))),
                "{cmd}: {:?}",
                i.warnings
            );
        }
        for cmd in [
            "curl https://x.test/a > out.json",
            "curl https://x.test/a >out.json",
            "curl https://x.test/a >> log.txt",
            "curl > out.json https://x.test/a",
            "curl https://x.test/a 2>&1",
            "curl https://x.test/a 2>/dev/null",
            "curl https://x.test/a < input.txt",
            "curl https://x.test/a &> /dev/null",
        ] {
            let i = ok(cmd);
            assert_eq!(i.request.url, "https://x.test/a", "{cmd}");
        }
        assert!(
            ok("curl https://x.test/a > out.json")
                .warnings
                .iter()
                .any(|w| w.contains("redirections"))
        );
        // Quoted operators, and `;` or `&` inside a word, are plain characters.
        let r = ok(r#"curl 'https://x.test/a?x=1;y=2&z=3' -d 'a | b && c ; d > e'"#).request;
        assert_eq!(r.url, "https://x.test/a?x=1;y=2&z=3");
        assert_eq!(r.body, "a | b && c ; d > e");
        assert_eq!(
            ok("curl https://x.test/a?x=1;y=2").request.url,
            "https://x.test/a?x=1;y=2"
        );
        assert_eq!(
            ok("curl https://x.test/a?x=1&y=2").request.url,
            "https://x.test/a?x=1&y=2"
        );
    }

    #[test]
    fn cmd_style_is_recognised_by_its_shape_not_by_apostrophes() {
        // The body contains an apostrophe, which used to disable the check.
        let cmd =
            "curl ^\"https://api.test/x^\" ^\n  -H ^\"a: b^\" ^\n  --data-raw ^\"^{^\\^\"n^\\^\":^\\^\"it's^\\^\"}^\"";
        assert!(parse(cmd).unwrap_err().contains("cmd-style"));
        // A bash command that merely contains `^"` is fine.
        let i = ok("curl https://a.test -d 'x^\"y' -H 'X: 1'");
        assert_eq!(i.request.body, "x^\"y");
        assert!(!looks_like_cmd_style("curl 'https://a.test' \\\n  -H 'a: b'"));
    }

    #[test]
    fn base64_matches_the_rfc_vectors() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), expected, "{input:?}");
        }
        assert_eq!(base64(&[0xff, 0xfe, 0xfd]), "//79");
    }

    #[test]
    fn dash_u_with_an_empty_username_survives_as_a_header() {
        // The Auth tab sends nothing for an empty username, so this must not become Auth::Basic.
        let i = ok("curl -u ':ghp_token' https://api.test");
        assert_eq!(i.request.auth, Auth::None);
        assert_eq!(
            header(&i.request, "authorization"),
            Some(format!("Basic {}", base64(b":ghp_token")).as_str())
        );
        assert!(
            i.warnings.iter().any(|w| w.contains("empty username")),
            "{:?}",
            i.warnings
        );
        // A normal username is unaffected.
        assert_eq!(
            ok("curl -u a:b https://api.test").request.auth,
            Auth::Basic {
                username: "a".into(),
                password: "b".into()
            }
        );
    }

    #[test]
    fn form_import_is_lossless_or_stays_raw() {
        let form = "-H 'Content-Type: application/x-www-form-urlencoded'";
        // Escapes that are not valid UTF-8, valueless keys and empty parts would change the wire.
        for data in ["f=%FF%FE&k=v", "a&b=1", "a=1&&b=2", "a=1&", "&a=1", "k=%E9"] {
            let r = ok(&format!("curl https://a.test {form} -d '{data}'")).request;
            assert_eq!(r.body_kind, BodyKind::Raw, "{data}");
            assert_eq!(r.body, data, "{data}");
            assert_eq!(
                header(&r, "content-type"),
                Some("application/x-www-form-urlencoded"),
                "header kept for {data}"
            );
        }
        // Without any Content-Type the same data also stays raw, with the label curl would send.
        let r = ok("curl https://a.test -d 'a=1&&b=2'").request;
        assert_eq!(r.body_kind, BodyKind::Raw);
        // A charset parameter is kept, so the data stays raw to keep that header.
        let r = ok("curl https://a.test -H 'Content-Type: application/x-www-form-urlencoded; charset=UTF-8' -d 'a=1'")
            .request;
        assert_eq!(r.body_kind, BodyKind::Raw);
        assert_eq!(
            header(&r, "content-type"),
            Some("application/x-www-form-urlencoded; charset=UTF-8")
        );
        // Clean data still becomes editable rows.
        let r = ok(&format!("curl https://a.test {form} -d 'a=1&b=%C3%A9'")).request;
        assert_eq!(r.body_kind, BodyKind::Form);
        assert_eq!(r.form, vec![KeyValue::new("a", "1"), KeyValue::new("b", "é")]);
    }

    #[test]
    fn prose_is_not_mistaken_for_a_curl_command() {
        for prose in [
            "curl is a command line tool",
            "curl that thing",
            "curl, the tool",
            "curl 42",
            "curl\nis nice",
        ] {
            assert!(!looks_like_curl(prose), "{prose:?}");
        }
        for cmd in [
            "curl example.com",
            "curl api:8080/health",
            "curl localhost:3000",
            "curl 127.0.0.1:8080/x",
            "curl 'https://a.test'",
            "curl \"https://a.test\"",
            "curl -s https://a.test",
            "curl --url https://a.test",
            "curl $URL",
            "curl \\\n  https://a.test",
            "curl\n  -H 'a: b' https://a.test",
        ] {
            assert!(looks_like_curl(cmd), "{cmd:?}");
        }
    }

    #[test]
    fn service_host_with_port_is_a_url_not_an_unknown_options_value() {
        assert!(looks_like_url("api:8080/health"));
        assert!(looks_like_url("my-service:3000"));
        assert!(!looks_like_url("api:abc"));
        assert!(!looks_like_url("u:p"));
        assert_eq!(
            ok("curl --retry-connrefused api:8080/health").request.url,
            "api:8080/health"
        );
        assert_eq!(
            ok("curl --some-unknown-flag value https://a.test").request.url,
            "https://a.test"
        );
    }

    #[test]
    fn data_options_imply_post_even_with_empty_or_file_data() {
        for cmd in [
            "curl -d '' https://a.test",
            "curl -d @payload.json https://a.test",
            "curl --data-raw '' https://a.test",
            "curl --json @body.json https://a.test",
            "curl --data-urlencode @f https://a.test",
        ] {
            let r = ok(cmd).request;
            assert_eq!(r.method, Method::Post, "{cmd}");
            assert_eq!(r.body_kind, BodyKind::None, "{cmd}");
        }
        // -G turns data into a query, and an explicit -X still wins.
        assert_eq!(ok("curl -G -d '' https://a.test").request.method, Method::Get);
        assert_eq!(ok("curl -X PUT -d '' https://a.test").request.method, Method::Put);
    }

    #[test]
    fn form_data_with_a_blank_key_stays_raw() {
        let form = "-H 'Content-Type: application/x-www-form-urlencoded'";
        for data in ["=1&a=2", "%20=1", "+=x", "a=1&=2"] {
            let r = ok(&format!("curl https://a.test {form} -d '{data}'")).request;
            assert_eq!(
                r.body_kind,
                BodyKind::Raw,
                "{data}: the send path would drop the blank-key row"
            );
            assert_eq!(r.body, data);
        }
    }

    #[test]
    fn only_the_first_of_several_curl_commands_is_imported() {
        let i = ok("curl -X POST https://a.test/one\ncurl -X GET https://b.test/two");
        assert_eq!(i.request.method, Method::Post);
        assert_eq!(i.request.url, "https://a.test/one");
        assert!(
            i.warnings.iter().any(|w| w.contains("Only the first curl command")),
            "{:?}",
            i.warnings
        );
        let i = ok("curl https://a.test/one \\\n  -H 'a: b'\n\n  curl.exe https://b.test");
        assert_eq!(i.request.url, "https://a.test/one");
        assert!(
            i.request.headers.iter().any(|h| h.key == "a"),
            "the continuation line still belongs to the first"
        );
        // A newline that is not followed by another command is just whitespace.
        let i = ok("curl https://a.test/one\n  -H 'a: b'");
        assert!(i.request.headers.iter().any(|h| h.key == "a"));
        assert!(i.warnings.is_empty());
    }

    #[test]
    fn windows_line_endings_are_recognised_and_parsed() {
        let cmd = "curl \\\r\n  'https://a.test/x' \\\r\n  -H 'a: b' \\\r\n  --compressed";
        assert!(looks_like_curl(cmd));
        let i = ok(cmd);
        assert_eq!(i.request.url, "https://a.test/x");
        assert!(i.request.headers.iter().any(|h| h.key == "a" && h.value == "b"));
    }

    #[test]
    fn several_cookie_options_are_joined_like_curl() {
        let r = ok("curl https://a.test -b a=1 -b 'b=2; c=3' --cookie d=4").request;
        let cookies: Vec<_> = r
            .headers
            .iter()
            .filter(|h| h.key.eq_ignore_ascii_case("cookie"))
            .collect();
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].value, "a=1; b=2; c=3; d=4");
        let r = ok("curl https://a.test -H 'Cookie: x=1' -b y=2").request;
        assert_eq!(header(&r, "cookie"), Some("x=1; y=2"));
    }

    #[test]
    fn explicit_headers_beat_dash_a_and_dash_e() {
        let r = ok("curl -A foo -H 'User-Agent: bar' -e https://r1.test -H 'referer: https://r2.test' https://a.test")
            .request;
        let uas: Vec<_> = r
            .headers
            .iter()
            .filter(|h| h.key.eq_ignore_ascii_case("user-agent"))
            .collect();
        assert_eq!(uas.len(), 1);
        assert_eq!(uas[0].value, "bar");
        let refs: Vec<_> = r
            .headers
            .iter()
            .filter(|h| h.key.eq_ignore_ascii_case("referer"))
            .collect();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].value, "https://r2.test");
        // Without an explicit header, -A and -e apply (once).
        let r = ok("curl -A foo -e https://r.test https://a.test").request;
        assert_eq!(header(&r, "user-agent"), Some("foo"));
        assert_eq!(header(&r, "referer"), Some("https://r.test"));
    }

    #[test]
    fn a_bare_host_after_an_unknown_flag_is_the_url() {
        assert_eq!(ok("curl --some-unknown-flag myserver").request.url, "myserver");
        assert_eq!(ok("curl -W myserver").request.url, "myserver");
        // With a URL elsewhere the word is the unknown option's value, as before.
        assert_eq!(
            ok("curl --some-unknown-flag value https://a.test").request.url,
            "https://a.test"
        );
        assert_eq!(
            ok("curl https://a.test --some-unknown-flag value").request.url,
            "https://a.test"
        );
        // Common boolean options are known, so a following bare host is never swallowed.
        for flag in [
            "-q",
            "-n",
            "-O",
            "-J",
            "-R",
            "-Z",
            "-j",
            "-0",
            "--http2-prior-knowledge",
            "--retry-connrefused",
        ] {
            assert_eq!(ok(&format!("curl {flag} myserver")).request.url, "myserver", "{flag}");
        }
    }

    #[test]
    fn percent_helpers_round_trip() {
        for s in ["", "plain", "a b&c=d", "é€😀", "100%", "+plus+"] {
            assert_eq!(percent_decode(&percent_encode(s)), s, "{s:?}");
        }
        assert_eq!(percent_decode("a+b%20c%zz%4"), "a b c%zz%4");
    }
}
