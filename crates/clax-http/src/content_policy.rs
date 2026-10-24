//! What a site host's responses may load and run: the
//! `Content-Security-Policy` that keeps an artifact's pages to their own
//! site ([`lockdown`]), and the service worker a site host answers for any
//! registration ([`SERVICE_WORKER_KILL_SWITCH`]). The site layer
//! ([`crate::host::site_layer`], through
//! [`crate::site_access::site_response_headers`], and the Worker's static
//! files through [`crate::site_access::site_headers`]) puts the lockdown on
//! every site response a content route did not give its own policy
//! ([`ContentPolicySet`]).

use axum::http::{HeaderMap, header};

/// A service worker script for any registration or update on a site host:
/// it installs, then unregisters itself, and handles no fetch. A page's own
/// worker script never runs (a worker answers its frames with documents
/// that carry no policy), and a worker a page registered before is replaced
/// and removed at its next update check.
pub const SERVICE_WORKER_KILL_SWITCH: &str = "self.addEventListener(\"install\",()=>self.skipWaiting());\
self.addEventListener(\"activate\",e=>e.waitUntil(self.registration.unregister()));";

/// Whether `req` asks for a service worker's script (`Service-Worker:
/// script`).
pub fn wants_service_worker(req: &HeaderMap) -> bool {
    req.get("service-worker")
        .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"script"))
}

/// The lockdown policy of a site response, after `frame` (its
/// `frame-ancestors`, [`crate::site_access::frame_ancestors`]): the page
/// loads its own site's files, the bridge and its parts, its assets
/// (`/_clax/blob/`), and `data:` and `blob:` URLs; it fetches (`fetch`,
/// XHR, WebSocket, EventSource, beacons, JSON modules) only its own site,
/// whose `/_clax/api/` is its runtime; no plugins, base URL or form posts.
/// Inline and evaluated scripts and inline styles run, being the page's
/// own. The network beyond its site is the `net` capability's, through the
/// shell. A page's own `<meta>` policy still applies in full beside it.
pub fn lockdown(frame: &str) -> String {
    format!(
        "{frame}; default-src 'self' data: blob:; \
         script-src 'self' 'unsafe-inline' 'unsafe-eval' data: blob:; \
         style-src 'self' 'unsafe-inline' data: blob:; \
         connect-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'"
    )
}

/// The request's `Host` when it is a plain host and port (letters, digits,
/// `.`, `-`, `:`, `[`, `]`), else `fallback` (the daemon's own host): a
/// crafted `Host` never reaches a policy.
pub fn plain_host<'a>(req: &'a HeaderMap, fallback: &'a str) -> &'a str {
    let plain = |h: &str| {
        !h.is_empty()
            && h.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b))
    };
    req.get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .filter(|h| plain(h))
        .unwrap_or(fallback)
}

/// Response extension: the content route set this response's policy itself
/// (a live page's snapshot keeps only its framing part and the snapshot's
/// own policy); the site layer adds the lockdown to every other.
#[derive(Clone, Copy)]
pub struct ContentPolicySet;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lockdown_keeps_a_page_to_its_own_site() {
        let p = lockdown("frame-ancestors 'self' clax://app");
        assert!(p.starts_with("frame-ancestors 'self' clax://app; "), "{p}");
        assert!(p.contains("connect-src 'self';"), "{p}");
        assert!(p.contains("form-action 'none'"), "{p}");
        assert!(!p.contains('*'), "{p}");
    }
}
