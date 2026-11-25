//! What `clax.web.search` and `clax.web.fetch` tell a script: what each
//! returns, and each refusal, in one run where web use stays as the run
//! found it and in a run of its own for each refusal that takes another
//! setting, source or run.

use super::names as g;
use super::pins::{
    ATTEMPT, GIVEN, Pins, driven, fetched, pin_normalised, price, price_the_web, sender_says, spent,
};
use super::scripts::script_result;
use crate::common::*;
use crate::resume::crash_once;
use clax_bot::run::CrashPoint;
use clax_bot::web::{
    FakeWebSource, FetchLimits, FetchedPage, Format, OwnHosts, PublicUrl, SearchAnswer, SearchHit,
    SearchQuery, WebError, WebShape, WebSource,
};
use clax_bot::{Next, Wait};
use clax_bot_sandbox::{CancelToken, HostError};
use clax_core::audit::AuditCtx;
use clax_store::bot_runs::{DriverId, RunStatus, StepKind};
use clax_store::bot_settings::Setting;
use clax_store::credits::LINE_WEB_FETCH;
use clax_store::roles::OWNER;
use serde_json::{Value, json};
use std::sync::Arc;

/// The one hit of the search `blue headings`.
pub const FOUND: &str = "https://example.org/found";

/// The answer to the search `blue headings`: one hit, [`FOUND`].
pub fn found() -> Result<SearchAnswer, WebError> {
    Ok(SearchAnswer {
        hits: vec![SearchHit {
            url: FOUND.into(),
            title: Some("Found".into()),
            description: Some("About blue headings".into()),
            age: None,
        }],
        request_id: Some("req-1".into()),
        raw: json!({"hits": [FOUND]}),
    })
}

/// `clax.web.search` and `clax.web.fetch` in one script of a run that
/// declared both: what each returns, and each refusal a script can meet
/// while web use stays as the run found it (wrong arguments, a URL that
/// is not public, not the sender's or not fetched in this format, and a
/// source that fails). [`web_refusals_are_pinned`] has the refusals that
/// take another setting, source or run.
#[tokio::test]
async fn the_web_operations_answers_are_pinned() {
    const SLOW: &str = "https://example.com/slow";
    const BLOCKED: &str = "https://example.com/blocked";
    const MOVED: &str = "http://example.com/moved";
    let env = Env::new().await;
    price_the_web(&env);
    let web = env.web();
    web.page(GIVEN, fetched(GIVEN));
    web.page(FOUND, fetched(FOUND));
    web.search_answers("blue headings", found());
    web.search_answers(
        "slow",
        Err(WebError::new(
            clax_bot::web::CODE_WEB_TIMEOUT,
            "the search took longer than 10 seconds",
            true,
        )),
    );
    web.page(
        SLOW,
        Err(WebError::new(
            clax_bot::web::CODE_WEB_TIMEOUT,
            "the page took longer than 10 seconds",
            true,
        )),
    );
    web.page(
        BLOCKED,
        Err(WebError::new(
            clax_bot::web::CODE_WEB_BLOCKED,
            "the site answered 403",
            false,
        )),
    );
    web.redirect(MOVED, "http://10.0.0.1/");
    sender_says(
        &env,
        &format!("Read {GIVEN}, {SLOW}, {BLOCKED} and {MOVED}"),
    );
    let code = format!(
        r##"{ATTEMPT}
await attempt("web.search", () => clax.web.search("blue headings"));
await attempt("web.fetch", () => clax.web.fetch("{GIVEN}"));
await attempt("web.fetch: a URL the run was not given", () => clax.web.fetch("https://elsewhere.example/x"));
await attempt("web.fetch: a private host", () => clax.web.fetch("http://127.0.0.1/admin"));
await attempt("web.fetch: not a URL", () => clax.web.fetch("not a url"));
await attempt("web.fetch: a negative offset", () => clax.web.fetch("{GIVEN}", {{offset: -1}}));
await attempt("web.fetch: an unknown format", () => clax.web.fetch("{GIVEN}", {{format: "html"}}));
await attempt("web.search: a limit of 0", () => clax.web.search("blue headings", 0));
await attempt("web.search: an empty query", () => clax.web.search(""));
await attempt("web.search: a query of 1,025 characters", () => clax.web.search("q".repeat(1025)));
await attempt("web.search: the source fails, retryable", () => clax.web.search("slow"));
await attempt("web.fetch: the same page again, from the run's store", () => clax.web.fetch("{GIVEN}", {{offset: 5}}));
await attempt("web.fetch: a page read as auto, asked for raw", () => clax.web.fetch("{GIVEN}", {{format: "raw"}}));
await attempt("web.fetch: a result of the search", () => clax.web.fetch("{FOUND}"));
await attempt("web.fetch: a URL that holds credentials", () => clax.web.fetch("https://user:secret@example.com/a"));
await attempt("web.fetch: a URL that looks credentialed and is not the sender's", () => clax.web.fetch("{FOUND}?api_key=1"));
await attempt("web.fetch: a URL of 2,001 characters", () => clax.web.fetch("https://example.com/" + "a".repeat(1981)));
await attempt("web.fetch: a scheme that is not fetched", () => clax.web.fetch("ftp://example.com/a"));
await attempt("web.fetch: localhost", () => clax.web.fetch("http://localhost:8080/"));
await attempt("web.fetch: the source fails, retryable", () => clax.web.fetch("{SLOW}"));
await attempt("web.fetch: the source fails, not retryable", () => clax.web.fetch("{BLOCKED}"));
await attempt("web.fetch: a redirect to a private host", () => clax.web.fetch("{MOVED}"));
await attempt("web.fetch: a URL of 1,020 characters that is 6,020 once encoded", () => clax.web.fetch("https://example.com/" + "é".repeat(1000)));
await attempt("web.fetch: an IPv6 loopback address", () => clax.web.fetch("http://[::1]/admin"));
return out;"##
    );
    env.api.push(env.script_turn("tu_1", "Web", &code));
    env.api.push(finish_turn("done"));
    let run = env.run_with_web("search,fetch");
    let p = driven(env, run).await;
    p.pin(
        "what a script's result tells a model (the web operations)",
        g::SCRIPT_WEB_OPERATIONS,
        &script_result(&p, 2),
    );
}

/// A web source that answers as `fake` does and says of itself what a
/// test set: the operations it can perform, why it cannot be used here,
/// and the instance's own hosts.
struct Told {
    fake: Arc<FakeWebSource>,
    configured: WebShape,
    unusable: Option<String>,
    own: OwnHosts,
}

impl Told {
    /// `env`'s source, able to perform both operations, usable, with no
    /// hosts of its own.
    fn of(env: &Env) -> Told {
        Told {
            fake: env.web(),
            configured: WebShape::BOTH,
            unusable: None,
            own: OwnHosts::default(),
        }
    }

    /// [`Told::of`], able to fetch and not to search.
    fn fetch_only(env: &Env) -> Told {
        Told {
            configured: WebShape {
                search: false,
                fetch: true,
            },
            ..Told::of(env)
        }
    }
}

#[async_trait::async_trait]
impl WebSource for Told {
    async fn search(
        &self,
        q: &SearchQuery,
        cancel: &CancelToken,
    ) -> Result<SearchAnswer, WebError> {
        self.fake.search(q, cancel).await
    }

    async fn fetch(
        &self,
        url: &PublicUrl,
        format: Format,
        limits: &FetchLimits,
        hop: &(dyn for<'u> Fn(&'u PublicUrl) -> Result<(), WebError> + Sync),
        cancel: &CancelToken,
    ) -> Result<FetchedPage, WebError> {
        self.fake.fetch(url, format, limits, hop, cancel).await
    }

    fn configured(&self) -> WebShape {
        self.configured
    }

    fn unusable(&self) -> Option<String> {
        self.unusable.clone()
    }

    fn own_hosts(&self) -> OwnHosts {
        self.own.clone()
    }
}

/// One run of [`web_refusals_are_pinned`]: a script that meets a refusal
/// for what the case sets.
struct Case {
    name: &'static str,
    /// The web operations the run declares.
    shape: &'static str,
    /// The script's `attempt` lines, after [`ATTEMPT`].
    code: String,
    /// What is set before the run starts: the instance's settings and
    /// prices, the source's pages, the sender's comment.
    before: fn(&Env),
    /// The source the run reaches, when it is not the environment's own.
    source: Option<fn(&Env) -> Told>,
    /// What changes while the run works: after its turn, before its
    /// script.
    mid: fn(&Env),
}

impl Case {
    /// A case of `name` whose run declares `shape` and whose script is
    /// `code`, with nothing set and the environment's own source.
    fn new(name: &'static str, shape: &'static str, code: impl Into<String>) -> Case {
        Case {
            name,
            shape,
            code: code.into(),
            before: |_| {},
            source: None,
            mid: |_| {},
        }
    }

    fn before(self, before: fn(&Env)) -> Case {
        Case { before, ..self }
    }

    fn source(self, source: fn(&Env) -> Told) -> Case {
        Case {
            source: Some(source),
            ..self
        }
    }

    fn mid(self, mid: fn(&Env)) -> Case {
        Case { mid, ..self }
    }

    /// Runs the case in an environment of its own, whose sender's comment
    /// names [`GIVEN`], which its web source answers as it answers the
    /// search `blue headings`, at [`price_the_web`]'s prices: the run's
    /// turn, then what changes, then the script. Answers the case's name
    /// and what the script was told ([`script_result`]), normalised.
    async fn run(self) -> Value {
        let env = Env::new().await;
        price_the_web(&env);
        env.web().page(GIVEN, fetched(GIVEN));
        env.web().search_answers("blue headings", found());
        sender_says(&env, &format!("Read {GIVEN}"));
        (self.before)(&env);
        let deps = match self.source {
            Some(source) => env.deps().with_web(Arc::new(source(&env))),
            None => env.deps(),
        };
        env.api.push(env.script_turn(
            "tu_1",
            "Web",
            &format!("{ATTEMPT}\n{}\nreturn out;", self.code),
        ));
        let run = env.run_with_web(self.shape);
        let d = DriverId::mint();
        assert_eq!(env.tick(&deps, &run, &d).await.kind, Some(StepKind::Turn));
        (self.mid)(&env);
        assert_eq!(env.tick(&deps, &run, &d).await.kind, Some(StepKind::Script));
        let p = Pins::new(env, run);
        json!({"case": self.name, "result": p.normalised(&script_result(&p, 2))})
    }
}

const SEARCH: &str = r#"await attempt("web.search", () => clax.web.search("blue headings"));"#;

/// The line that fetches [`GIVEN`].
fn fetch() -> String {
    format!(r#"await attempt("web.fetch", () => clax.web.fetch("{GIVEN}"));"#)
}

// The instance's own hosts: its public origin, a site of it, and a page
// elsewhere that redirects to it.
const OWN: &str = "https://clax.example/a/x";
const SITE: &str = "https://abcdefghijkl.sites.example/";
const AWAY: &str = "https://example.com/r";

// The admin's list of hosts: a host on it, which redirects to one off it.
const LISTED: &str = "https://docs.example.com/a";
const OFF: &str = "https://other.example/c";

/// The cases of [`web_refusals_are_pinned`], in the golden's order.
fn refusals() -> Vec<Case> {
    let both = format!("{SEARCH}\n{}", fetch());
    vec![
        // The caps: one search and one fetch a run.
        Case::new(
            "the run's caps: 1 search and 1 fetch",
            "search,fetch",
            format!(
                r#"{both}
await attempt("web.search: past the run's cap", () => clax.web.search("blue headings"));
await attempt("web.fetch: past the run's cap", () => clax.web.fetch("{FOUND}"));
await attempt("web.fetch: a page the run's store keeps, at the cap", () => clax.web.fetch("{GIVEN}"));"#
            ),
        )
        .before(|env| {
            env.admin_sets(&[
                (Setting::WebMaxSearchesPerRun, json!(1)),
                (Setting::WebMaxFetchesPerRun, json!(1)),
            ]);
            env.web().page(FOUND, fetched(FOUND));
        }),
        // An operation the run did not declare.
        Case::new("a run that declared fetch alone", "fetch", SEARCH),
        Case::new("a run that declared search alone", "search", fetch()),
        // Turned off since the run started.
        Case::new(
            "the admin turned search off since the run started",
            "search,fetch",
            both.clone(),
        )
        .mid(|env| env.admin_sets(&[(Setting::WebSearchEnabled, json!(false))])),
        Case::new(
            "the admin turned fetch off since the run started",
            "search,fetch",
            both.clone(),
        )
        .mid(|env| env.admin_sets(&[(Setting::WebFetchEnabled, json!(false))])),
        Case::new(
            "the admin turned both off since the run started",
            "search,fetch",
            both.clone(),
        )
        .mid(|env| {
            env.admin_sets(&[
                (Setting::WebSearchEnabled, json!(false)),
                (Setting::WebFetchEnabled, json!(false)),
            ]);
        }),
        Case::new(
            "the person turned web use off since the run started",
            "search,fetch",
            both.clone(),
        )
        .mid(|env| {
            env.store
                .set_web_choice(&AuditCtx::DAEMON, OWNER, false)
                .unwrap();
        }),
        Case::new(
            "a source that can no longer search",
            "search,fetch",
            both.clone(),
        )
        .source(Told::fetch_only),
        Case::new(
            "a source that can no longer search, and the admin turned fetch off",
            "search,fetch",
            fetch(),
        )
        .source(Told::fetch_only)
        .mid(|env| env.admin_sets(&[(Setting::WebFetchEnabled, json!(false))])),
        // A source that cannot be used where the host runs.
        Case::new(
            "a source that cannot be used here",
            "search,fetch",
            both.clone(),
        )
        .source(|env| Told {
            unusable: Some("Workers AI cannot run here.".into()),
            ..Told::of(env)
        }),
        // Pricing and credits.
        Case::new(
            "an instance without the web cost lines",
            "search,fetch",
            both.clone(),
        )
        .before(|env| {
            for line in ["web.search.ceramic", LINE_WEB_FETCH] {
                env.store.delete_cost_line(&AuditCtx::DAEMON, line).unwrap();
            }
        }),
        Case::new(
            "a person with 1 credit left, and a search that costs 5",
            "search",
            SEARCH,
        )
        .before(|env| price(env, "web.search.ceramic", 5_000))
        .mid(|env| {
            env.admin_sets(&[(Setting::FreeWeeklyMilli, json!(spent(env) + 1_000))]);
        }),
        // A request that cannot be read: the first fetch alone.
        Case::new(
            "a request that cannot be read",
            "fetch",
            format!(
                r#"{}
await attempt("web.fetch: again, the request readable", async () => (await clax.web.fetch("{GIVEN}")).status);"#,
                fetch()
            ),
        )
        .mid(|env| {
            env.host.lock().fail_next.insert(
                "context".into(),
                HostError::new("unavailable", "the store is busy"),
            );
        }),
        // The instance's own hosts, asked for and redirected to.
        Case::new(
            "the instance's own hosts",
            "fetch",
            format!(
                r#"await attempt("web.fetch: the instance's public origin", () => clax.web.fetch("{OWN}"));
await attempt("web.fetch: a site of the instance", () => clax.web.fetch("{SITE}"));
await attempt("web.fetch: a redirect to the instance's public origin", () => clax.web.fetch("{AWAY}"));"#
            ),
        )
        .before(|env| {
            sender_says(env, &format!("Read {OWN}, {SITE} and {AWAY}"));
            env.web().redirect(AWAY, "https://clax.example/admin");
        })
        .source(|env| Told {
            own: OwnHosts::new(Some("https://clax.example"), Some("sites.example")).unwrap(),
            ..Told::of(env)
        }),
        // The admin's list of hosts, on the URL asked for and on a redirect.
        Case::new(
            "an instance that fetches from *.example.com alone",
            "fetch",
            format!(
                r#"await attempt("web.fetch: a host off the list", () => clax.web.fetch("{OFF}"));
await attempt("web.fetch: a redirect to a host off the list", () => clax.web.fetch("{LISTED}"));"#
            ),
        )
        .before(|env| {
            env.admin_sets(&[
                (Setting::WebFetchUrlRule, json!("allowlist")),
                (Setting::WebFetchAllowlist, json!(["*.example.com"])),
            ]);
            sender_says(env, &format!("Read {LISTED} and {OFF}"));
            env.web().redirect(LISTED, OFF);
        }),
        // Any public URL: one the run was not given is fetched.
        Case::new(
            "an instance that fetches any public URL",
            "fetch",
            format!(
                r#"await attempt("web.fetch: a URL the run was not given (its status)", async () => (await clax.web.fetch("{FOUND}")).status);
await attempt("web.fetch: a URL that looks credentialed and is not the sender's", () => clax.web.fetch("{FOUND}?token=1"));"#
            ),
        )
        .before(|env| {
            env.admin_sets(&[(Setting::WebFetchUrlRule, json!("any_public"))]);
            env.web().page(FOUND, fetched(FOUND));
        }),
    ]
}

/// The web refusals that take a setting, a source or a run other than
/// [`the_web_operations_answers_are_pinned`]'s, each in a run of its own
/// ([`refusals`]): the caps, an operation the run did not declare, web
/// use turned off since the run started (by the admin, by the person, by
/// a source that lost what it needs), a source that cannot be used here,
/// pricing that is not configured, credits that do not pay for the call,
/// a request that cannot be read, the instance's own hosts, and hosts off
/// the admin's list, on the URL asked for and on a redirect.
#[tokio::test]
async fn web_refusals_are_pinned() {
    let mut cases = Vec::new();
    for case in refusals() {
        cases.push(case.run().await);
    }
    pin_normalised(
        "what a script is told when the web refuses it (each case a run of its own)",
        g::SCRIPT_WEB_REFUSALS,
        &Value::Array(cases),
    );
}

/// The journal rows and the step's `calls` of the web operations a run
/// lacks, each in a run of its own that is crashed before the script
/// step's commit and resumed: a run that declared neither operation,
/// and a run that declared both on a host with no web source. Each
/// refusal's row is the run loop's own form, the failure's `code` and
/// `message` and nothing else, written once by the first attempt; what
/// the run shows of the call (`cost_milli: 0` and `web`) is in the
/// step's `calls`, the same for the attempt that was answered again.
#[tokio::test]
async fn the_journal_rows_of_web_operations_a_run_lacks_are_pinned() {
    let mut cases = Vec::new();
    for (name, shape, source) in [
        ("a run that declared no web operation", "", true),
        (
            "a run that declared both, on a host with no web source",
            "search,fetch",
            false,
        ),
    ] {
        let env = Env::new().await;
        env.api.push(env.script_turn(
            "tu_1",
            "Web",
            &format!("{ATTEMPT}\n{SEARCH}\n{}\nreturn out;", fetch()),
        ));
        env.api.push(finish_turn("done"));
        let run = env.run_with_web(shape);
        let deps = || {
            let mut deps = env.deps();
            if !source {
                deps = deps.without_web();
            }
            deps
        };
        let mut dying = deps();
        crash_once(
            &mut dying,
            CrashPoint::BeforeCommit {
                step: 2,
                kind: StepKind::Script,
            },
        );
        let out = env.drive(&dying, &run, &DriverId::mint()).await;
        assert_eq!(out.last().unwrap().next, Next::Waiting(Wait::Crashed));
        env.clock.advance(61);
        let out = env.drive(&deps(), &run, &DriverId::mint()).await;
        assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
        assert!(
            env.web().calls().is_empty(),
            "{name}: the source was reached"
        );
        let p = Pins::new(env, run);
        let calls = p.script_calls();
        for row in calls[0]["journal"].as_array().expect("the journal") {
            let keys: Vec<&String> = row["result"]
                .as_object()
                .expect("a failure")
                .keys()
                .collect();
            assert_eq!(keys, ["code", "message"], "{name}: {row}");
        }
        cases.push(json!({
            "case": name,
            "calls": p.normalised(&calls),
            "result": p.normalised(&script_result(&p, 2)),
        }));
    }
    pin_normalised(
        "what is kept of the web operations a run lacks (each case a run of its own): the \
         step's `calls`, its journal rows and what the script is told",
        g::JOURNAL_WEB_DISABLED,
        &Value::Array(cases),
    );
}
