use reqwest::Method;

/// A single HTTP request, independent of how it was built (CLI flags today,
/// a saved collection entry later). No variable substitution happens here —
/// by the time a `Request` exists, `url`/`headers`/`body` are final strings.
#[derive(Debug, Clone)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Body>,
}

#[derive(Debug, Clone)]
pub enum Body {
    Raw(String),
}

impl Request {
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn with_body(mut self, body: Body) -> Self {
        self.body = Some(body);
        self
    }
}
