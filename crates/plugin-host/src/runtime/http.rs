//! REST requests: the client's requests to Discord run past the
//! `http-filter`s before they are sent, and their responses before the
//! client reads them (design doc §22).
//!
//! A plugin sees a request only on a route whose kind it registered with
//! `host-events.intercept-http` and can read. The record carries no
//! headers, and routes whose bodies or paths carry credentials (`/auth`,
//! `/users/@me`, billing, `oauth2`, webhooks, interactions) have no kind, so
//! no plugin ever sees them.

use super::Runtime;
use crate::api;
use crate::bindings::ex::http_filter as hf;
use crate::model;

/// The largest body a plugin sees or may send back, in bytes.
pub const MAX_BODY: usize = 256 * 1024;
/// How many `http-filter` calls a plugin gets a minute, requests and
/// responses together. Past it, traffic passes without calling it.
pub const CALLS_PER_MINUTE: usize = 300;

/// A request the client is about to send, as the backend sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestRequest {
    /// Unique per request, so a response can be paired with it.
    pub id: u64,
    pub method: String,
    /// The path below `/api/v{n}`, with the query.
    pub path: String,
    pub body: Option<String>,
}

/// What the backend does with a request after the filters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestOutcome {
    /// Send it with this body.
    Send(Option<String>),
    /// Don't send it; the client sees a failed request.
    Drop,
}

/// The kinds whose routes the host knows.
pub(super) fn routable(kind: api::EventKind) -> bool {
    use api::EventKind as K;
    matches!(
        kind,
        K::Message
            | K::Reaction
            | K::PollVote
            | K::Pins
            | K::Typing
            | K::Member
            | K::Relationship
            | K::Guild
            | K::Channel
            | K::Role
            | K::Emoji
    )
}

/// What a REST route is about, from its path below the API version. `None`
/// for every route not listed in the design doc's table, which includes
/// every route that carries credentials.
#[must_use]
pub fn route_kind(path: &str) -> Option<api::EventKind> {
    use api::EventKind as K;
    let path = path.split(['?', '#']).next().unwrap_or("");
    let seg: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let id = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    match seg.as_slice() {
        ["channels", c, rest @ ..] if id(c) => match rest {
            [] => Some(K::Channel),
            ["messages", "pins", ..] | ["pins", ..] => Some(K::Pins),
            ["messages", m, "reactions", ..] if id(m) => Some(K::Reaction),
            ["messages", ..] => Some(K::Message),
            ["polls", ..] => Some(K::PollVote),
            ["typing"] => Some(K::Typing),
            _ => None,
        },
        ["guilds", g, rest @ ..] if id(g) => match rest {
            [] => Some(K::Guild),
            ["channels"] => Some(K::Channel),
            ["members", ..] => Some(K::Member),
            ["roles", ..] => Some(K::Role),
            ["emojis", ..] => Some(K::Emoji),
            _ => None,
        },
        ["users", "@me", "relationships", ..] => Some(K::Relationship),
        _ => None,
    }
}

/// A body a plugin may see: present, UTF-8 by construction, within
/// [`MAX_BODY`].
fn visible(body: Option<&String>) -> bool {
    body.is_none_or(|b| b.len() <= MAX_BODY)
}

/// A replacement body the host accepts: JSON within [`MAX_BODY`].
fn acceptable(body: &str) -> bool {
    body.len() <= MAX_BODY && serde_json::from_str::<serde_json::Value>(body).is_ok()
}

impl Runtime {
    /// Run a request past every `http-filter` that sees its route, before
    /// the client sends it. Plugins see it in load order, each getting the
    /// body the one before returned; the first `drop` ends it.
    pub fn filter_request(&mut self, req: &RestRequest) -> RequestOutcome {
        let Some(kind) = route_kind(&req.path) else {
            return RequestOutcome::Send(req.body.clone());
        };
        let mut body = req.body.clone();
        for idx in 0..self.plugins.len() {
            if !self.http_filters(idx, kind) || !visible(body.as_ref()) || !self.take_http_call(idx)
            {
                continue;
            }
            let record = hf::HttpRequest {
                id: req.id,
                method: req.method.clone(),
                path: req.path.clone(),
                kind,
                body: body.clone(),
            };
            let fuel = self.config.limits.fuel_per_render;
            let verdict = self.call_with_fuel(idx, fuel, |h, store| match &h.http {
                Some(f) => f.call_on_request(store, &record),
                None => Ok(hf::RequestVerdict::Pass),
            });
            match verdict {
                Some(hf::RequestVerdict::Drop) => return RequestOutcome::Drop,
                Some(hf::RequestVerdict::Replace(new)) if acceptable(&new) => body = Some(new),
                Some(hf::RequestVerdict::Replace(_)) => self.http_warn(idx, "request"),
                Some(hf::RequestVerdict::Pass) | None => {}
            }
        }
        RequestOutcome::Send(body)
    }

    /// Run the response to `req` past every `http-filter` that sees its
    /// route, before the client reads it. Returns the body the client reads.
    pub fn filter_response(
        &mut self,
        req: &RestRequest,
        status: u16,
        body: Option<String>,
    ) -> Option<String> {
        let Some(kind) = route_kind(&req.path) else {
            return body;
        };
        let mut body = body;
        for idx in 0..self.plugins.len() {
            if !self.http_filters(idx, kind) || !visible(body.as_ref()) || !self.take_http_call(idx)
            {
                continue;
            }
            let record = hf::HttpResponse {
                id: req.id,
                method: req.method.clone(),
                path: req.path.clone(),
                kind,
                status,
                body: body.clone(),
            };
            let fuel = self.config.limits.fuel_per_render;
            let verdict = self.call_with_fuel(idx, fuel, |h, store| match &h.http {
                Some(f) => f.call_on_response(store, &record),
                None => Ok(hf::ResponseVerdict::Pass),
            });
            match verdict {
                Some(hf::ResponseVerdict::Replace(new)) if acceptable(&new) => body = Some(new),
                Some(hf::ResponseVerdict::Replace(_)) => self.http_warn(idx, "response"),
                Some(hf::ResponseVerdict::Pass) | None => {}
            }
        }
        body
    }

    /// Whether any active plugin's `http-filter` sees requests on routes of
    /// `kind`. A backend checks this before building the record.
    pub fn filters_http(&self, kind: api::EventKind) -> bool {
        (0..self.plugins.len()).any(|idx| self.http_filters(idx, kind))
    }

    /// Whether plugin `idx` is active, exports `http-filter`, registered
    /// `kind` and can still read it.
    fn http_filters(&self, idx: usize, kind: api::EventKind) -> bool {
        let p = &self.plugins[idx];
        let st = p.store.data();
        p.status == super::Status::Active
            && p.hooks.http.is_some()
            && st.http_intercepts.contains(&(kind as u8))
            && st.grants.has(super::convert::kind_capability(kind))
    }

    /// Count one `http-filter` call against plugin `idx`'s rate.
    fn take_http_call(&mut self, idx: usize) -> bool {
        let st = self.plugins[idx].store.data_mut();
        let now = st.now();
        st.rates.http.take(now, CALLS_PER_MINUTE, 60_000)
    }

    fn http_warn(&mut self, idx: usize, what: &str) {
        let id = self.plugins[idx].manifest.id.clone();
        let msg = format!(
            "http-filter replaced a {what} body with one that isn't JSON or is over {} KiB; \
             passing the original",
            MAX_BODY / 1024
        );
        self.host_log(&id, model::LogLevel::Warn, &msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use api::EventKind as K;

    #[test]
    fn routes_map_to_kinds() {
        assert_eq!(
            route_kind("/channels/1/messages?limit=50"),
            Some(K::Message)
        );
        assert_eq!(route_kind("/channels/1/messages/2"), Some(K::Message));
        assert_eq!(
            route_kind("/channels/1/messages/2/reactions/%F0%9F%91%8D/@me"),
            Some(K::Reaction)
        );
        assert_eq!(route_kind("/channels/1/messages/pins/2"), Some(K::Pins));
        assert_eq!(route_kind("/channels/1/pins"), Some(K::Pins));
        assert_eq!(route_kind("/channels/1/typing"), Some(K::Typing));
        assert_eq!(
            route_kind("/channels/1/polls/2/answers/1"),
            Some(K::PollVote)
        );
        assert_eq!(route_kind("/channels/1"), Some(K::Channel));
        assert_eq!(route_kind("/guilds/3"), Some(K::Guild));
        assert_eq!(route_kind("/guilds/3/channels"), Some(K::Channel));
        assert_eq!(route_kind("/guilds/3/members/4"), Some(K::Member));
        assert_eq!(route_kind("/guilds/3/roles"), Some(K::Role));
        assert_eq!(route_kind("/guilds/3/emojis/5"), Some(K::Emoji));
        assert_eq!(
            route_kind("/users/@me/relationships/6"),
            Some(K::Relationship)
        );
    }

    #[test]
    fn routes_that_carry_credentials_have_no_kind() {
        for path in [
            "/auth/login",
            "/auth/mfa/totp",
            "/users/@me",
            "/users/@me/mfa/totp/enable",
            "/users/@me/billing/payment-sources",
            "/oauth2/authorize?client_id=1",
            "/webhooks/1/secret-token",
            "/webhooks/1/secret-token/messages/2",
            "/interactions/1/secret-token/callback",
            "/channels/not-an-id/messages",
            "/guilds/3/bans/4",
        ] {
            assert_eq!(route_kind(path), None, "{path}");
        }
    }

    #[test]
    fn replacements_must_be_json_within_the_limit() {
        assert!(acceptable(r#"{"content":"hi"}"#));
        assert!(acceptable("[]"));
        assert!(!acceptable("not json"));
        let big = format!("\"{}\"", "a".repeat(MAX_BODY));
        assert!(!acceptable(&big));
        assert!(visible(None));
        assert!(!visible(Some(&"a".repeat(MAX_BODY + 1))));
    }
}
