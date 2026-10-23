//! The cookies of the artifacts' sites, kept by the core for the app's
//! content frames (spec 2026-10-21-clax-run-site-origins, "The access
//! handshake"). A frame reaches its site through the app's scheme, whose
//! answers no WebView keeps a cookie of (a `WKURLSchemeHandler`'s and an
//! intercepted Android request's alike), so the core keeps each site's
//! cookies in the browser's place: what a site's answer sets, per site,
//! sent with every later request of a content frame of that site. The
//! handshake then runs as the web shell's does: the shell asks the core
//! for a code (`POST /api/artifacts/<id>/site-code`, as the owner), the
//! frame loads `/_clax/session/start` (which sets the state cookie here)
//! and `/_clax/session?code=…` (which redeems it beside that state and
//! sets the site cookie here).
//!
//! Only the site cookies' names and values are kept; their attributes
//! (`HttpOnly`, `Path=/`, `SameSite`, `Partitioned`) hold by construction:
//! no page script reads the jar, every one is the site's root path, and
//! only the frames of the site the cookie came from send it. A cookie set
//! with `Max-Age=0` (or no value) is removed. The jar lives as long as the
//! core: closing it ends every site session's cookie, as quitting a browser
//! ends a session cookie.

use axum::http::{HeaderMap, header};
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

/// The cookies the core keeps per site (by artifact ID).
#[derive(Default)]
pub(crate) struct SiteJar(Mutex<HashMap<String, BTreeMap<String, String>>>);

impl SiteJar {
    /// The `Cookie` header for a request of site `id`'s content frame, or
    /// `None` when the jar holds none of it.
    pub(crate) fn cookie_header(&self, id: &str) -> Option<String> {
        let jar = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let cookies = jar.get(id).filter(|c| !c.is_empty())?;
        Some(
            cookies
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("; "),
        )
    }

    /// Keeps the cookies `headers` (an answer to site `id`'s content frame)
    /// set, and removes those it ends.
    pub(crate) fn take(&self, id: &str, headers: &HeaderMap) {
        let set: Vec<(String, Option<String>)> = headers
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .filter_map(parse)
            .collect();
        if set.is_empty() {
            return;
        }
        let mut jar = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let cookies = jar.entry(id.to_string()).or_default();
        for (name, value) in set {
            match value {
                Some(v) => {
                    cookies.insert(name, v);
                }
                None => {
                    cookies.remove(&name);
                }
            }
        }
    }
}

/// A `Set-Cookie` value's name and value; `None` for the value when it
/// ends the cookie (`Max-Age` 0 or less, or an empty value).
fn parse(set: &str) -> Option<(String, Option<String>)> {
    let mut parts = set.split(';');
    let (name, value) = parts.next()?.split_once('=')?;
    let (name, value) = (name.trim(), value.trim());
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return None;
    }
    let ended = value.is_empty()
        || parts.any(|a| {
            a.trim().split_once('=').is_some_and(|(k, v)| {
                k.trim().eq_ignore_ascii_case("max-age")
                    && v.trim().parse::<i64>().is_ok_and(|n| n <= 0)
            })
        });
    Some((name.to_string(), (!ended).then(|| value.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn set(values: &[&'static str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for v in values {
            h.append(header::SET_COOKIE, HeaderValue::from_static(v));
        }
        h
    }

    #[test]
    fn a_sites_cookies_go_to_its_frames_alone_and_end_when_told() {
        let jar = SiteJar::default();
        assert_eq!(jar.cookie_header("abcdefghjkmn"), None);
        jar.take(
            "abcdefghjkmn",
            &set(&[
                "clax_site_state=s1; Path=/; HttpOnly; Max-Age=600",
                "clax_site=k1; Path=/; HttpOnly; SameSite=None; Secure; Partitioned",
            ]),
        );
        assert_eq!(
            jar.cookie_header("abcdefghjkmn").as_deref(),
            Some("clax_site=k1; clax_site_state=s1")
        );
        assert_eq!(jar.cookie_header("zzzzzzzzzzzz"), None);
        jar.take(
            "abcdefghjkmn",
            &set(&["clax_site_state=; Path=/; Max-Age=0"]),
        );
        assert_eq!(
            jar.cookie_header("abcdefghjkmn").as_deref(),
            Some("clax_site=k1")
        );
        // A malformed one changes nothing.
        jar.take("abcdefghjkmn", &set(&["=x", "a b=c", "noequals"]));
        assert_eq!(
            jar.cookie_header("abcdefghjkmn").as_deref(),
            Some("clax_site=k1")
        );
    }
}
