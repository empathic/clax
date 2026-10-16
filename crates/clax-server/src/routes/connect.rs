//! `GET /connect/b/<code>`: the browser link (spec §6.5). `clax connect`
//! opens it in the connected person's browser, which becomes that person:
//! the page sets the `clax_viewer` cookie to a new secret naming the
//! person's viewer (never its row ID) and moves
//! on to `/people`. A `clax_viewer` cookie the browser already carried is
//! replaced, never folded, so a raced or sniffed link moves no one's
//! history. The code is spent on its first use; it is refused for the owner
//! (whose grants have no code), and refused without being spent from a
//! browser on the daemon's own machine (a loopback peer naming a local
//! host) and for a navigation another page made (`Sec-Fetch-Site` other
//! than `none` or `same-origin`), so no page can make a browser someone
//! else.
//!
//! The code is in the URL, so every answer is `no-store` and sends no
//! `Referer` onward.

use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

/// Length of a browser-link code: 32 bytes in unpadded base64url.
const CODE_LEN: usize = 43;

fn page(status: StatusCode, title: &str, text: &str, next: Option<&str>) -> Response {
    let refresh = next
        .map(|u| format!(r#"<meta http-equiv="refresh" content="0;url={u}">"#))
        .unwrap_or_default();
    let link = next
        .map(|u| format!(r#"<p><a href="{u}">Continue</a></p>"#))
        .unwrap_or_default();
    let html = format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="referrer" content="no-referrer">{refresh}<title>{title}</title></head><body><p>{text}</p>{link}</body></html>"#
    );
    let mut res = (status, html).into_response();
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'"),
    );
    res
}

fn gone() -> Response {
    page(
        StatusCode::NOT_FOUND,
        "Link not valid",
        "This link was used already, has expired, or is not valid. Run clax connect again for a new one.",
        None,
    )
}

/// `GET /connect/b/<code>`: 200 with a page that sets the `clax_viewer`
/// cookie to the person's viewer and moves on to `/people`; 404 for a code
/// that is unknown, used, expired, or whose grant or person has ended; 403
/// from the daemon's own machine or from another page.
pub async fn browser_link(
    State(s): State<AppState>,
    _d: crate::agent::DeniedToGrants,
    code: Result<Path<String>, axum::extract::rejection::PathRejection>,
    req: axum::extract::Request,
) -> Response {
    if crate::identity::is_local(req.headers(), crate::identity::peer_of(req.extensions())) {
        return page(
            StatusCode::FORBIDDEN,
            "Open on the connected machine",
            "This link joins the browser of the machine that connected. Open it there.",
            None,
        );
    }
    // `clax connect` opens the link from outside any page (`Sec-Fetch-Site:
    // none`); a page that sends a browser here (a link, a script) would
    // make that browser someone else's (login CSRF).
    if crate::viewer::fetched_from_elsewhere(req.headers()) {
        return page(
            StatusCode::FORBIDDEN,
            "Open from clax connect",
            "This link works only when clax connect opens it. Run clax connect again on your machine.",
            None,
        );
    }
    let Ok(Path(code)) = code else {
        return gone();
    };
    if code.len() != CODE_LEN
        || !code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return gone();
    }
    match s.store_call(move |st| st.redeem_browser_code(&code)).await {
        Ok((_, cookie)) => {
            let mut res = page(
                StatusCode::OK,
                "Connected",
                "This browser is now yours in Clax.",
                Some("/people"),
            );
            res.headers_mut()
                .insert(header::SET_COOKIE, crate::viewer::set_cookie(&cookie));
            res
        }
        Err(e) if e.status == StatusCode::NOT_FOUND => gone(),
        Err(e) => {
            tracing::warn!(code = e.code, "browser link failed");
            page(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Something went wrong",
                "Clax could not open this link. Try again.",
                None,
            )
        }
    }
}
