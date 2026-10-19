use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct KeyValue {
    pub enabled: bool,
    pub key: String,
    pub value: String,
}

impl KeyValue {
    pub fn new(key: &str, value: &str) -> Self {
        Self {
            enabled: true,
            key: key.into(),
            value: value.into(),
        }
    }
}

impl Default for KeyValue {
    fn default() -> Self {
        Self::new("", "")
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Method {
    #[default]
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
}

impl Method {
    pub const ALL: [Method; 7] = [
        Method::Get,
        Method::Post,
        Method::Put,
        Method::Patch,
        Method::Delete,
        Method::Head,
        Method::Options,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
            Method::Head => "HEAD",
            Method::Options => "OPTIONS",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BodyKind {
    #[default]
    None,
    Raw,
    Form,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub enum Auth {
    #[default]
    None,
    Bearer {
        token: String,
    },
    Basic {
        username: String,
        password: String,
    },
    ApiKey {
        name: String,
        value: String,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Request {
    pub name: String,
    pub method: Method,
    pub url: String,
    pub params: Vec<KeyValue>,
    pub headers: Vec<KeyValue>,
    pub body_kind: BodyKind,
    pub body: String,
    pub form: Vec<KeyValue>,
    pub auth: Auth,
}

impl Request {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Case-insensitive match on name, URL or method; an empty query matches everything.
    pub fn matches(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        q.is_empty()
            || self.name.to_lowercase().contains(&q)
            || self.url.to_lowercase().contains(&q)
            || self.method.as_str().to_lowercase().contains(&q)
    }

    /// Label for tabs and lists: the name, falling back to method + URL.
    pub fn title(&self) -> String {
        if !self.name.trim().is_empty() {
            self.name.clone()
        } else if !self.url.trim().is_empty() {
            format!("{} {}", self.method.as_str(), self.url)
        } else {
            "Untitled".into()
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Collection {
    pub name: String,
    pub requests: Vec<Request>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Environment {
    pub name: String,
    pub vars: Vec<KeyValue>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    pub fn next(self) -> Self {
        match self {
            ThemeMode::System => ThemeMode::Light,
            ThemeMode::Light => ThemeMode::Dark,
            ThemeMode::Dark => ThemeMode::System,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::System => "System",
            ThemeMode::Light => "Light",
            ThemeMode::Dark => "Dark",
        }
    }
}

/// Where the response panel sits relative to the request editor.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SplitLayout {
    #[default]
    Below,
    Side,
}

impl SplitLayout {
    pub fn toggled(self) -> Self {
        match self {
            SplitLayout::Below => SplitLayout::Side,
            SplitLayout::Side => SplitLayout::Below,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SplitLayout::Below => "Response below",
            SplitLayout::Side => "Response beside",
        }
    }
}

/// Look-and-feel settings that survive restarts.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct UiPrefs {
    pub theme: ThemeMode,
    pub split: SplitLayout,
    pub sidebar_width: Option<f32>,
    pub response_height: Option<f32>,
    pub response_width: Option<f32>,
}

pub const HISTORY_LIMIT: usize = 50;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Store {
    pub collections: Vec<Collection>,
    pub environments: Vec<Environment>,
    pub active_env: Option<usize>,
    pub history: Vec<Request>,
    pub timeout_secs: Option<u64>,
    pub ui: UiPrefs,
}

impl Store {
    pub fn push_history(&mut self, req: &Request) {
        self.history.retain(|r| r != req);
        self.history.insert(0, req.clone());
        self.history.truncate(HISTORY_LIMIT);
    }

    pub fn active_vars(&self) -> &[KeyValue] {
        self.active_env
            .and_then(|i| self.environments.get(i))
            .map(|e| e.vars.as_slice())
            .unwrap_or(&[])
    }

    /// Keeps `active_env` valid after environments are removed.
    pub fn fix_active_env(&mut self) {
        if let Some(i) = self.active_env
            && i >= self.environments.len()
        {
            self.active_env = None;
        }
    }
}
