use std::path::PathBuf;

use reqwest::Method;

/// A single HTTP request, independent of how it was built (CLI flags today,
/// a saved collection entry later). No variable substitution happens here —
/// by the time a `Request` exists, `url`/`headers`/`body` are final strings.
#[derive(Debug, Clone)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub query_params: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: Option<Body>,
    pub auth: Option<Auth>,
}

#[derive(Debug, Clone)]
pub enum Body {
    /// Sent as-is as the request body (e.g. a JSON string via -d).
    Raw(String),
    /// Raw bytes, e.g. read from a file via --data-binary.
    Binary(Vec<u8>),
    /// application/x-www-form-urlencoded key/value pairs.
    Form(Vec<(String, String)>),
    /// multipart/form-data fields.
    Multipart(Vec<MultipartField>),
}

#[derive(Debug, Clone)]
pub enum MultipartField {
    Text { name: String, value: String },
    File { name: String, path: PathBuf },
}

#[derive(Debug, Clone)]
pub enum Auth {
    Basic { username: String, password: String },
    Bearer { token: String },
}

impl Request {
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            query_params: Vec::new(),
            headers: Vec::new(),
            body: None,
            auth: None,
        }
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn with_query(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.query_params.push((name.into(), value.into()));
        self
    }

    pub fn with_body(mut self, body: Body) -> Self {
        self.body = Some(body);
        self
    }

    pub fn with_auth(mut self, auth: Auth) -> Self {
        self.auth = Some(auth);
        self
    }
}
