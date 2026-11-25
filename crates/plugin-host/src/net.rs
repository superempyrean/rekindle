//! Outbound HTTPS for plugins with the `network` capability.
//!
//! Policy is enforced here, before any transport is touched:
//! - `https://` only;
//! - the URL host must be on the plugin's approved allowlist (exact match),
//!   or the allowlist is `*` and the host is a public name: a name with a
//!   dot, not under `localhost`, `local`, `internal` or `home.arpa`;
//! - redirects are never followed (the 3xx is returned to the plugin);
//! - request/response bodies and total time are capped;
//! - the host never adds cookies or credentials. There is no cookie jar and
//!   no API through which a plugin could read the user's token.

use std::collections::BTreeSet;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
            Method::Head => "HEAD",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HttpError {
    #[error("host `{0}` is not on this plugin's allowlist")]
    HostNotAllowed(String),
    #[error("only https:// URLs are allowed")]
    InsecureScheme,
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("request timed out")]
    Timeout,
    #[error("response too large")]
    TooLarge,
    #[error("transport error: {0}")]
    Transport(String),
}

#[derive(Debug, Clone, Copy)]
pub struct NetLimits {
    pub timeout: Duration,
    pub max_request_bytes: usize,
    pub max_response_bytes: usize,
}

impl Default for NetLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(15),
            max_request_bytes: 1024 * 1024,
            max_response_bytes: 4 * 1024 * 1024,
        }
    }
}

/// Request headers a plugin may not set: hop-by-hop or ones the transport
/// owns. (A plugin *may* send its own `Authorization` for a third-party API:
/// that's its own key, not the user's.)
const FORBIDDEN_REQUEST_HEADERS: &[&str] = &[
    "host",
    "connection",
    "content-length",
    "transfer-encoding",
    "upgrade",
    "te",
    "trailer",
    "keep-alive",
    "proxy-authorization",
    "cookie",
];

/// Check a request against the allowlist and return the normalized host.
pub fn check_request(
    req: &HttpRequest,
    allowlist: &BTreeSet<String>,
    limits: &NetLimits,
) -> Result<String, HttpError> {
    let url = url::Url::parse(&req.url).map_err(|e| HttpError::InvalidUrl(e.to_string()))?;
    if url.scheme() != "https" {
        return Err(HttpError::InsecureScheme);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(HttpError::InvalidUrl(
            "credentials in URL are not allowed".into(),
        ));
    }
    let host = match url.host() {
        Some(url::Host::Domain(d)) => d.to_ascii_lowercase(),
        Some(_) => {
            return Err(HttpError::HostNotAllowed(
                url.host_str().unwrap_or_default().to_string(),
            ));
        }
        None => return Err(HttpError::InvalidUrl("missing host".into())),
    };
    let any = allowlist.contains(crate::manifest::ANY_HOST) && is_public_name(&host);
    if !any && !allowlist.contains(&host) {
        return Err(HttpError::HostNotAllowed(host));
    }
    if req
        .body
        .as_ref()
        .is_some_and(|b| b.len() > limits.max_request_bytes)
    {
        return Err(HttpError::InvalidUrl("request body too large".into()));
    }
    if let Some((name, _)) = req
        .headers
        .iter()
        .find(|(n, _)| FORBIDDEN_REQUEST_HEADERS.contains(&n.to_ascii_lowercase().as_str()))
    {
        return Err(HttpError::InvalidUrl(format!(
            "header `{name}` may not be set by plugins"
        )));
    }
    Ok(host)
}

/// Whether `host` names a machine on the internet rather than on the user's
/// own network: it has a dot and isn't under a name kept for local use.
fn is_public_name(host: &str) -> bool {
    const LOCAL: [&str; 4] = ["localhost", "local", "internal", "home.arpa"];
    let host = host.trim_end_matches('.');
    host.contains('.')
        && !LOCAL
            .iter()
            .any(|l| host == *l || host.ends_with(&format!(".{l}")))
}

/// The transport. Called only after [`check_request`] passed.
pub trait NetClient: Send + Sync {
    fn send(&self, req: &HttpRequest, limits: &NetLimits) -> Result<HttpResponse, HttpError>;
}

/// Real HTTPS via `ureq` (rustls). No cookie jar, no redirects, no proxy auth.
pub struct UreqClient;

impl NetClient for UreqClient {
    fn send(&self, req: &HttpRequest, limits: &NetLimits) -> Result<HttpResponse, HttpError> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_global(Some(limits.timeout))
            .user_agent("rekindle-plugin")
            .build()
            .into();

        let mut builder = ureq::http::Request::builder()
            .method(req.method.as_str())
            .uri(&req.url);
        for (k, v) in &req.headers {
            builder = builder.header(k, v);
        }
        let request = builder
            .body(req.body.clone().unwrap_or_default())
            .map_err(|e| HttpError::InvalidUrl(e.to_string()))?;

        let mut resp = agent.run(request).map_err(|e| match e {
            ureq::Error::Timeout(_) => HttpError::Timeout,
            other => HttpError::Transport(other.to_string()),
        })?;
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_string(),
                    String::from_utf8_lossy(v.as_bytes()).into_owned(),
                )
            })
            .collect();
        let body = if req.method == Method::Head {
            Vec::new()
        } else {
            resp.body_mut()
                .with_config()
                .limit(limits.max_response_bytes as u64)
                .read_to_vec()
                .map_err(|e| match e {
                    ureq::Error::BodyExceedsLimit(_) => HttpError::TooLarge,
                    ureq::Error::Timeout(_) => HttpError::Timeout,
                    other => HttpError::Transport(other.to_string()),
                })?
        };
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

/// Canned responses keyed by `METHOD url`, for tests and offline dev. Records
/// every request it sees so tests can assert on what a plugin sent.
#[derive(Default)]
pub struct MockNet {
    routes: std::sync::Mutex<Vec<(Method, String, HttpResponse)>>,
    seen: std::sync::Mutex<Vec<HttpRequest>>,
}

impl MockNet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn route(&self, method: Method, url: &str, status: u16, body: impl Into<Vec<u8>>) {
        self.routes.lock().unwrap().push((
            method,
            url.to_string(),
            HttpResponse {
                status,
                headers: vec![],
                body: body.into(),
            },
        ));
    }

    pub fn requests(&self) -> Vec<HttpRequest> {
        self.seen.lock().unwrap().clone()
    }
}

impl NetClient for MockNet {
    fn send(&self, req: &HttpRequest, limits: &NetLimits) -> Result<HttpResponse, HttpError> {
        self.seen.lock().unwrap().push(req.clone());
        let routes = self.routes.lock().unwrap();
        let hit = routes
            .iter()
            .rev()
            .find(|(m, u, _)| *m == req.method && *u == req.url);
        match hit {
            Some((_, _, r)) if r.body.len() > limits.max_response_bytes => Err(HttpError::TooLarge),
            Some((_, _, r)) => Ok(r.clone()),
            None => Err(HttpError::Transport(format!(
                "mock: no route for {} {}",
                req.method.as_str(),
                req.url
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(url: &str) -> HttpRequest {
        HttpRequest {
            method: Method::Get,
            url: url.into(),
            headers: vec![],
            body: None,
        }
    }

    #[test]
    fn enforces_scheme_host_and_headers() {
        let allow = BTreeSet::from(["api.example.com".to_string()]);
        let l = NetLimits::default();
        assert_eq!(
            check_request(&req("https://API.example.com/x"), &allow, &l),
            Ok("api.example.com".into())
        );
        assert_eq!(
            check_request(&req("http://api.example.com/"), &allow, &l),
            Err(HttpError::InsecureScheme)
        );
        assert!(matches!(
            check_request(&req("https://evil.example/"), &allow, &l),
            Err(HttpError::HostNotAllowed(_))
        ));
        assert!(matches!(
            check_request(&req("https://api.example.com.evil.example/"), &allow, &l),
            Err(HttpError::HostNotAllowed(_))
        ));
        assert!(matches!(
            check_request(&req("https://1.2.3.4/"), &allow, &l),
            Err(HttpError::HostNotAllowed(_))
        ));
        assert!(check_request(&req("https://u:p@api.example.com/"), &allow, &l).is_err());
        let mut r = req("https://api.example.com/");
        r.headers.push(("Cookie".into(), "a=b".into()));
        assert!(check_request(&r, &allow, &l).is_err());
        let mut r = req("https://api.example.com/");
        r.headers
            .push(("Authorization".into(), "Bearer plugin-own-key".into()));
        assert!(check_request(&r, &allow, &l).is_ok());
    }

    #[test]
    fn any_host_reaches_public_names_only() {
        let any = BTreeSet::from(["*".to_string()]);
        let l = NetLimits::default();
        for ok in ["https://evil.example/", "https://API.Example.com/x", "https://discord.com/api"] {
            assert!(check_request(&req(ok), &any, &l).is_ok(), "{ok}");
        }
        for refused in [
            "https://localhost/",
            "https://printer.local/",
            "https://router.home.arpa/",
            "https://db.internal/",
            "https://a.localhost/",
            "https://intranet/",
            "https://1.2.3.4/",
            "https://[::1]/",
        ] {
            assert!(
                matches!(check_request(&req(refused), &any, &l), Err(HttpError::HostNotAllowed(_))),
                "{refused}"
            );
        }
        assert_eq!(
            check_request(&req("http://evil.example/"), &any, &l),
            Err(HttpError::InsecureScheme)
        );
    }
}
