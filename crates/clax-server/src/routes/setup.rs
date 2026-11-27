//! `GET /setup/feedback`: the instructions a coding agent follows to set up
//! in-app bug reports for an app on this Clax, as Markdown, for any caller
//! with no credential. The page is `docs/setup-feedback.md`, with this
//! instance's origin, kind and name filled in from what `GET
//! /.well-known/clax` says of it ([`super::discovery::describe`]), which
//! links to it (`setup.feedback`).

use axum::extract::State;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use clax_http::agent::DeniedToGrants;
use clax_http::error::ApiError;
use clax_http::state::AppState;

/// The page's path, which `/.well-known/clax` names as `setup.feedback`.
pub const FEEDBACK_PATH: &str = "/setup/feedback";

/// The page as written, with `{{origin}}`, `{{kind}}` and `{{name}}` to fill.
const FEEDBACK: &str = include_str!("../../../../docs/setup-feedback.md");

/// `GET /setup/feedback` → 200 `text/markdown; charset=utf-8`: the setup
/// page with this instance's origin, kind and name, the same for every
/// caller.
pub async fn feedback(State(s): State<AppState>, _d: DeniedToGrants) -> Result<Response, ApiError> {
    let d = super::discovery::describe(&s);
    let body = render(FEEDBACK, &d.origin, &d.kind, &d.name);
    let mut r = body.into_response();
    let h = r.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/markdown; charset=utf-8"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    Ok(r)
}

/// `page` without its leading comment (the note to its editors), with each
/// placeholder filled: a name's line breaks and backticks are dropped, as
/// it stands inside a line of Markdown.
fn render(page: &str, origin: &str, kind: &str, name: &str) -> String {
    let page = match (page.find("<!--"), page.find("-->")) {
        (Some(a), Some(b)) if a < b => {
            let rest = page[b + 3..].trim_start_matches('\n');
            format!("{}{rest}", &page[..a])
        }
        _ => page.to_string(),
    };
    let name: String = name
        .chars()
        .filter(|c| !c.is_control() && *c != '`' && *c != '*')
        .collect();
    page.replace("{{origin}}", origin)
        .replace("{{kind}}", kind)
        .replace("{{name}}", &name)
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn every_placeholder_is_filled_and_the_editors_note_left_out() {
        let page = render(
            FEEDBACK,
            "https://clax.example",
            "hosted",
            "Studio `Clax`\n",
        );
        assert!(!page.contains("{{"), "{page}");
        assert!(!page.contains("<!--"), "{page}");
        assert!(page.starts_with("# Set up Clax feedback"), "{page}");
        assert!(page.contains("**Studio Clax**"), "{page}");
        assert!(page.contains("--instance https://clax.example"), "{page}");
    }
}
