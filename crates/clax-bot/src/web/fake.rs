//! [`FakeWebSource`]: a scripted [`WebSource`] for tests, beside
//! [`crate::host::FakeBotHost`]. The Worker's and the daemon's tests load
//! theirs from JSON fixtures ([`FakeWebSource::from_fixtures`]).

use super::{
    CODE_URL_NOT_ALLOWED, CODE_WEB_UNAVAILABLE, FetchLimits, FetchedPage, Format, PublicUrl,
    SearchAnswer, SearchHit, SearchQuery, WebError, WebSource, url::redirect_allowed,
};
use clax_bot_sandbox::{CODE_CANCELLED, CancelToken};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

/// A [`WebSource`] that answers what a test scripted, records every call,
/// and answers `web_unavailable` to anything unscripted.
///
/// Answers to one query or URL are given in the order they were scripted;
/// the last one is repeated once the others are used. A URL is matched by
/// its normalised form ([`PublicUrl::normalized`]). A redirect
/// ([`FakeWebSource::redirect`]) is followed as a real source follows one:
/// an `https` to `http` hop is refused, `hop` is called on the target, and
/// a chain longer than [`FetchLimits::max_redirects`] is `web_unavailable`.
#[derive(Debug, Default)]
pub struct FakeWebSource {
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    searches: HashMap<String, VecDeque<Result<SearchAnswer, WebError>>>,
    pages: HashMap<String, VecDeque<Result<FetchedPage, WebError>>>,
    redirects: HashMap<String, String>,
    calls: Vec<(String, String)>,
}

/// `url`'s normalised form, or `url` itself when it is not a public URL.
fn key(url: &str) -> String {
    PublicUrl::parse(url).map_or_else(|_| url.to_string(), |u| u.normalized().to_string())
}

/// The next of `answers`: the first, or the last one left.
fn next<T: Clone>(answers: Option<&mut VecDeque<T>>) -> Option<T> {
    let answers = answers?;
    if answers.len() > 1 {
        answers.pop_front()
    } else {
        answers.front().cloned()
    }
}

fn cancelled() -> WebError {
    WebError::new(CODE_CANCELLED, "the run was cancelled", false)
}

fn unscripted(what: &str) -> WebError {
    WebError::new(
        CODE_WEB_UNAVAILABLE,
        format!("the fake web source has no answer for {what}"),
        false,
    )
}

impl FakeWebSource {
    #[must_use]
    pub fn new() -> FakeWebSource {
        FakeWebSource::default()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Scripts the next answer to a search for `query`.
    pub fn search_answers(&self, query: &str, r: Result<SearchAnswer, WebError>) {
        self.state()
            .searches
            .entry(query.to_string())
            .or_default()
            .push_back(r);
    }

    /// Scripts the next answer to a fetch of `url`.
    pub fn page(&self, url: &str, r: Result<FetchedPage, WebError>) {
        self.state().pages.entry(key(url)).or_default().push_back(r);
    }

    /// Makes a fetch of `from` redirect to `to`.
    pub fn redirect(&self, from: &str, to: &str) {
        self.state().redirects.insert(key(from), to.to_string());
    }

    /// Every call so far, in order: `("search", query)` and
    /// `("fetch", normalised URL)`, one per call (a redirect it followed is
    /// not another call).
    #[must_use]
    pub fn calls(&self) -> Vec<(String, String)> {
        self.state().calls.clone()
    }

    /// A source scripted from `v`:
    ///
    /// ```json
    /// {"searches": {"<query>": [{"url": "…", "title": "…", "description": "…", "age": "…"}]},
    ///  "pages": {"<url>": {"status": 200, "content_type": "text/html", "text": "…",
    ///                      "links": ["…"]},
    ///            "<url>": {"redirect": "<url>"}}}
    /// ```
    ///
    /// A missing `searches` or `pages` is none; a hit's `title`,
    /// `description` and `age` and a page's `links` are optional. A page
    /// with `redirect` redirects there ([`FakeWebSource::redirect`]); any
    /// other page needs `status`, `content_type` and `text`, and is
    /// answered with its own URL, normalised, as its final URL.
    ///
    /// # Errors
    /// What in `v` does not have this shape.
    pub fn from_fixtures(v: &Value) -> Result<FakeWebSource, String> {
        let Value::Object(top) = v else {
            return Err("fixtures must be an object".into());
        };
        let fake = FakeWebSource::new();
        let str_field = |o: &serde_json::Map<String, Value>, name: &str, at: &str| match o.get(name)
        {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(format!("{at}: {name} must be a string")),
        };
        if let Some(searches) = top.get("searches") {
            let searches = searches
                .as_object()
                .ok_or("searches must be an object of query to hits")?;
            for (query, hits) in searches {
                let at = format!("searches[{query:?}]");
                let hits = hits
                    .as_array()
                    .ok_or_else(|| format!("{at} must be an array of hits"))?;
                let hits = hits
                    .iter()
                    .enumerate()
                    .map(|(i, hit)| {
                        let at = format!("{at}[{i}]");
                        let o = hit
                            .as_object()
                            .ok_or_else(|| format!("{at} must be an object"))?;
                        Ok(SearchHit {
                            url: str_field(o, "url", &at)?
                                .ok_or_else(|| format!("{at}: url is required"))?,
                            title: str_field(o, "title", &at)?,
                            description: str_field(o, "description", &at)?,
                            age: str_field(o, "age", &at)?,
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                fake.search_answers(
                    query,
                    Ok(SearchAnswer {
                        hits,
                        request_id: None,
                        raw: searches[query].clone(),
                    }),
                );
            }
        }
        if let Some(pages) = top.get("pages") {
            let pages = pages
                .as_object()
                .ok_or("pages must be an object of URL to page")?;
            for (url, page) in pages {
                let at = format!("pages[{url:?}]");
                let o = page
                    .as_object()
                    .ok_or_else(|| format!("{at} must be an object"))?;
                if let Some(to) = str_field(o, "redirect", &at)? {
                    fake.redirect(url, &to);
                    continue;
                }
                let status = o
                    .get("status")
                    .and_then(Value::as_u64)
                    .and_then(|s| u16::try_from(s).ok())
                    .ok_or_else(|| format!("{at}: status must be an HTTP status"))?;
                let required = |name: &str| {
                    str_field(o, name, &at)?.ok_or_else(|| format!("{at}: {name} is required"))
                };
                let links = match o.get("links") {
                    None | Some(Value::Null) => Vec::new(),
                    Some(Value::Array(a)) => a
                        .iter()
                        .map(|l| {
                            l.as_str()
                                .map(str::to_string)
                                .ok_or_else(|| format!("{at}: links must be strings"))
                        })
                        .collect::<Result<_, _>>()?,
                    Some(_) => return Err(format!("{at}: links must be an array")),
                };
                fake.page(
                    url,
                    Ok(FetchedPage {
                        final_url: key(url),
                        status,
                        content_type: required("content_type")?,
                        text: required("text")?,
                        links,
                        request_id: None,
                    }),
                );
            }
        }
        Ok(fake)
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl WebSource for FakeWebSource {
    async fn search(
        &self,
        q: &SearchQuery,
        cancel: &CancelToken,
    ) -> Result<SearchAnswer, WebError> {
        let mut state = self.state();
        state.calls.push(("search".into(), q.query.clone()));
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        next(state.searches.get_mut(&q.query))
            .unwrap_or_else(|| Err(unscripted(&format!("the search {:?}", q.query))))
    }

    async fn fetch(
        &self,
        url: &PublicUrl,
        _format: Format,
        limits: &FetchLimits,
        hop: &(dyn for<'u> Fn(&'u PublicUrl) -> Result<(), WebError> + Sync),
        cancel: &CancelToken,
    ) -> Result<FetchedPage, WebError> {
        self.state()
            .calls
            .push(("fetch".into(), url.normalized().to_string()));
        let mut at = url.clone();
        let mut hops = 0u8;
        loop {
            if cancel.is_cancelled() {
                return Err(cancelled());
            }
            let to = self.state().redirects.get(at.normalized()).cloned();
            let Some(to) = to else { break };
            if hops >= limits.max_redirects {
                return Err(WebError::new(
                    CODE_WEB_UNAVAILABLE,
                    format!(
                        "{} redirected more than {} times",
                        url.normalized(),
                        limits.max_redirects
                    ),
                    false,
                ));
            }
            hops += 1;
            let to = PublicUrl::parse(&to)?;
            if !redirect_allowed(&at, &to) {
                return Err(WebError::new(
                    CODE_URL_NOT_ALLOWED,
                    format!("{} redirected from https to http", at.normalized()),
                    false,
                ));
            }
            hop(&to)?;
            at = to;
        }
        next(self.state().pages.get_mut(at.normalized()))
            .unwrap_or_else(|| Err(unscripted(&format!("a fetch of {}", at.normalized()))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::Provider;
    use serde_json::json;

    fn query(q: &str) -> SearchQuery {
        SearchQuery {
            query: q.into(),
            limit: 5,
            provider: Provider::Ceramic,
            gateway: "gw".into(),
            byok_alias: None,
        }
    }

    fn answer(urls: &[&str]) -> SearchAnswer {
        SearchAnswer {
            hits: urls
                .iter()
                .map(|u| SearchHit {
                    url: (*u).into(),
                    title: None,
                    description: None,
                    age: None,
                })
                .collect(),
            request_id: None,
            raw: json!({}),
        }
    }

    fn page(url: &str, text: &str) -> FetchedPage {
        FetchedPage {
            final_url: url.into(),
            status: 200,
            content_type: "text/plain".into(),
            text: text.into(),
            links: vec![],
            request_id: None,
        }
    }

    fn url(s: &str) -> PublicUrl {
        PublicUrl::parse(s).unwrap()
    }

    fn any(_: &PublicUrl) -> Result<(), WebError> {
        Ok(())
    }

    async fn fetch(
        f: &FakeWebSource,
        u: &str,
        hop: &(dyn Fn(&PublicUrl) -> Result<(), WebError> + Sync),
    ) -> Result<FetchedPage, WebError> {
        f.fetch(
            &url(u),
            Format::Auto,
            &FetchLimits::default(),
            hop,
            &CancelToken::new(),
        )
        .await
    }

    #[tokio::test]
    async fn the_fake_answers_in_order_and_records_calls() {
        let f = FakeWebSource::new();
        let c = CancelToken::new();
        f.search_answers("q", Ok(answer(&["https://a.com/"])));
        f.search_answers("q", Ok(answer(&["https://b.com/"])));
        f.page("https://A.com/p#x", Ok(page("https://a.com/p", "one")));
        f.page(
            "https://a.com/p",
            Err(WebError::new(
                crate::web::CODE_WEB_BLOCKED,
                "challenge",
                false,
            )),
        );
        f.redirect("https://r.com/", "https://a.com/p");
        f.redirect("https://down.com/", "http://a.com/p");

        let first = f.search(&query("q"), &c).await.unwrap();
        assert_eq!(first.hits[0].url, "https://a.com/");
        let second = f.search(&query("q"), &c).await.unwrap();
        assert_eq!(second.hits[0].url, "https://b.com/");
        // The last answer repeats.
        let third = f.search(&query("q"), &c).await.unwrap();
        assert_eq!(third.hits[0].url, "https://b.com/");
        let none = f.search(&query("other"), &c).await.unwrap_err();
        assert_eq!(none.code, CODE_WEB_UNAVAILABLE);

        assert_eq!(
            fetch(&f, "https://a.com/p", &any).await.unwrap().text,
            "one"
        );
        let blocked = fetch(&f, "https://a.com/p", &any).await.unwrap_err();
        assert_eq!(blocked.code, crate::web::CODE_WEB_BLOCKED);
        let missing = fetch(&f, "https://nowhere.com/", &any).await.unwrap_err();
        assert_eq!(missing.code, CODE_WEB_UNAVAILABLE);

        // A redirect calls `hop` on its target, and answers its refusal.
        let hops = Mutex::new(Vec::new());
        let record = |u: &PublicUrl| {
            hops.lock().unwrap().push(u.normalized().to_string());
            Ok(())
        };
        let redirected = fetch(&f, "https://r.com/", &record).await.unwrap_err();
        assert_eq!(redirected.code, crate::web::CODE_WEB_BLOCKED);
        assert_eq!(*hops.lock().unwrap(), vec!["https://a.com/p"]);
        let refuse = |_: &PublicUrl| Err(WebError::new(CODE_URL_NOT_ALLOWED, "no", false));
        let refused = fetch(&f, "https://r.com/", &refuse).await.unwrap_err();
        assert_eq!(refused.code, CODE_URL_NOT_ALLOWED);
        // An https to http redirect is refused before `hop`.
        let down = fetch(&f, "https://down.com/", &any).await.unwrap_err();
        assert_eq!(down.code, CODE_URL_NOT_ALLOWED);

        assert_eq!(
            f.calls(),
            [
                ("search", "q"),
                ("search", "q"),
                ("search", "q"),
                ("search", "other"),
                ("fetch", "https://a.com/p"),
                ("fetch", "https://a.com/p"),
                ("fetch", "https://nowhere.com/"),
                ("fetch", "https://r.com/"),
                ("fetch", "https://r.com/"),
                ("fetch", "https://down.com/"),
            ]
            .map(|(a, b)| (a.to_string(), b.to_string()))
        );
    }

    #[tokio::test]
    async fn a_redirect_chain_past_the_limit_is_unavailable() {
        let f = FakeWebSource::new();
        f.redirect("https://a.com/", "https://b.com/");
        f.redirect("https://b.com/", "https://a.com/");
        let e = fetch(&f, "https://a.com/", &any).await.unwrap_err();
        assert_eq!(e.code, CODE_WEB_UNAVAILABLE);
        assert_eq!(f.calls().len(), 1);
    }

    #[tokio::test]
    async fn a_cancelled_call_answers_cancelled() {
        let f = FakeWebSource::new();
        f.search_answers("q", Ok(answer(&[])));
        let c = CancelToken::new();
        c.cancel();
        assert_eq!(
            f.search(&query("q"), &c).await.unwrap_err().code,
            CODE_CANCELLED
        );
    }

    #[tokio::test]
    async fn fixtures_load_searches_pages_and_redirects() {
        let f = FakeWebSource::from_fixtures(&json!({
            "searches": {"rust": [
                {"url": "https://rust-lang.org/", "title": "Rust", "description": "A language", "age": "1d"},
                {"url": "https://docs.rs/"}
            ]},
            "pages": {
                "https://rust-lang.org/": {"status": 200, "content_type": "text/html",
                                           "text": "# Rust", "links": ["https://docs.rs/"]},
                "https://old.rust-lang.org/": {"redirect": "https://rust-lang.org/"}
            }
        }))
        .unwrap();
        let c = CancelToken::new();
        let a = f.search(&query("rust"), &c).await.unwrap();
        assert_eq!(
            a.hits,
            vec![
                SearchHit {
                    url: "https://rust-lang.org/".into(),
                    title: Some("Rust".into()),
                    description: Some("A language".into()),
                    age: Some("1d".into()),
                },
                SearchHit {
                    url: "https://docs.rs/".into(),
                    title: None,
                    description: None,
                    age: None,
                },
            ]
        );
        let p = fetch(&f, "https://old.rust-lang.org/", &any).await.unwrap();
        assert_eq!(
            p,
            FetchedPage {
                final_url: "https://rust-lang.org/".into(),
                status: 200,
                content_type: "text/html".into(),
                text: "# Rust".into(),
                links: vec!["https://docs.rs/".into()],
                request_id: None,
            }
        );

        let empty = FakeWebSource::from_fixtures(&json!({})).unwrap();
        assert_eq!(
            empty.search(&query("rust"), &c).await.unwrap_err().code,
            CODE_WEB_UNAVAILABLE
        );
        for bad in [
            json!([]),
            json!({"searches": {"q": [{"title": "no url"}]}}),
            json!({"pages": {"https://x.com/": {"status": 200}}}),
            json!({"pages": {"https://x.com/": {"status": 200, "content_type": "text/html",
                                               "text": "t", "links": [1]}}}),
        ] {
            assert!(FakeWebSource::from_fixtures(&bad).is_err(), "{bad}");
        }
    }
}
