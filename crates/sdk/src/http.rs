//! Outbound HTTPS (requires `network`).
//!
//! Only the hosts listed under `network` in the manifest are reachable. The
//! host never adds cookies or the user's credentials, and never follows
//! redirects: a 3xx comes back as it is.

use crate::raw::rekindle::plugin::http as raw;
pub use raw::{HttpError, Method};

/// A response to [`Request::send`].
#[derive(Debug, Clone)]
pub struct Response(raw::Response);

impl Response {
    /// The status code.
    pub fn status(&self) -> u16 {
        self.0.status
    }

    /// Whether the status is 2xx.
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.0.status)
    }

    /// The first header named `name`, compared case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.0
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The body.
    pub fn bytes(&self) -> &[u8] {
        &self.0.body
    }

    /// The body as UTF-8, with invalid sequences replaced.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.body).into_owned()
    }
}

/// A request, built up and then sent with [`Request::send`].
#[derive(Debug, Clone)]
pub struct Request(raw::Request);

impl Request {
    /// A request with no headers and no body.
    pub fn new(method: Method, url: &str) -> Self {
        Self(raw::Request {
            method,
            url: url.into(),
            headers: Vec::new(),
            body: None,
        })
    }

    /// Adds a header.
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.0.headers.push((name.into(), value.into()));
        self
    }

    /// Sets the body.
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.0.body = Some(body.into());
        self
    }

    /// Sends the request and waits for the response. An HTTP error status
    /// is a response, not an `Err`.
    pub fn send(&self) -> Result<Response, HttpError> {
        raw::fetch(&self.0).map(Response)
    }
}

/// Sends a `GET` request to `url`.
pub fn get(url: &str) -> Result<Response, HttpError> {
    Request::new(Method::Get, url).send()
}

/// Sends a `POST` request to `url` with `body`.
pub fn post(url: &str, body: impl Into<Vec<u8>>) -> Result<Response, HttpError> {
    Request::new(Method::Post, url).body(body).send()
}
