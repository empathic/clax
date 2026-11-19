//! [`ClaxProvider`]: Clax's own surface, provided to the run loop
//! through the seam ([`crate::surface::SurfaceProvider`]).

use super::session::{ClaxSession, State};
use super::tally::Tally;
use super::{browser, web};
use crate::api;
use crate::browser::BrowserSource;
use crate::browser::session::SessionLimits;
use crate::draft::Draft;
use crate::host::BotHost;
use crate::run::bot_ctx;
use crate::surface::{
    Answer, Binding, BindingAsStored, End, Fields, RunInfo, StepCx, StepSession, SurfaceProvider,
};
use crate::web::WebSource;
use clax_bot_sandbox::HostError;
use clax_core::CoreError;
use clax_core::audit::AuditIds;
use clax_core::working::Clock;
use clax_store::Store;
use clax_store::bot_content::ContentLimits;
use clax_store::bot_runs::Run;
use clax_surface::Definition;
use serde_json::Value;
use std::sync::{Arc, Mutex};

/// Clax's own surface (`src/clax.d.ts`): a run's draft of its artifact,
/// its threads, the artifact's documents, the web and the browser, over
/// the server's [`BotHost`] and the store.
///
/// A session ([`SurfaceProvider::begin`]) loads the run's draft at the
/// base the committed steps left it and performs each call as
/// [`crate::surface`] says, row by row ("Today's script calls, behind a
/// session"). The provider reads the run's own row (its artifact, its
/// threads, its request kind) by the run's ID, unless whoever made it
/// gave it the row ([`ClaxProvider::for_run`]), as a tick does
/// ([`SurfaceProvider::for_run`]).
///
/// It resolves a run's request to the artifact and the threads or the
/// brief as the host reads them now ([`BotHost::context`],
/// [`crate::prompt::request_document`]), files a run's audit records on
/// the artifact of its binding's scope, and, when a driver starts,
/// starts the follow-ups a crash left queued
/// ([`BotHost::start_waiting`]).
///
/// `working`, `ending` and `ended` are the roster on the run's threads,
/// what a run's end leaves on them and the follow-ups it starts
/// ([`super::ending`]).
pub struct ClaxProvider {
    pub(super) store: Arc<Store>,
    pub(super) host: Arc<dyn BotHost>,
    /// Where web calls are sent; `None` on a host without a source,
    /// where every web call is refused `web_disabled`.
    pub(super) web: Option<Arc<dyn WebSource>>,
    /// Where browsers come from; `None` on a host without a source,
    /// where every browser call is refused `browser_disabled`.
    pub(super) browser: Option<Arc<dyn BrowserSource>>,
    /// What a browser test is held to: its screenshots' longest side.
    pub(super) browser_limits: SessionLimits,
    /// What a browser test's screenshots are kept under, as the host
    /// holds a run's content ([`clax_store::Store::put_bot_content`]).
    pub(super) content_limits: ContentLimits,
    /// The clock of credits weeks.
    pub(super) clock: Arc<dyn Clock>,
    /// The row of the run the provider was made for, when it was made
    /// for one.
    run: Option<Run>,
}

impl std::fmt::Debug for ClaxProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaxProvider")
            .field("web", &self.web.is_some())
            .field("browser", &self.browser.is_some())
            .field("run", &self.run.as_ref().map(|r| &r.id))
            .finish_non_exhaustive()
    }
}

impl ClaxProvider {
    /// Clax's surface over `store` and `host`, sending web calls to
    /// `web` when the host has a source, with `clock` for the credits
    /// week.
    #[must_use]
    pub fn new(
        store: Arc<Store>,
        host: Arc<dyn BotHost>,
        web: Option<Arc<dyn WebSource>>,
        clock: Arc<dyn Clock>,
    ) -> ClaxProvider {
        ClaxProvider {
            store,
            host,
            web,
            browser: None,
            browser_limits: SessionLimits {
                image_side: crate::policy::DEFAULT_IMAGE_SIDE,
            },
            content_limits: ContentLimits::default(),
            clock,
            run: None,
        }
    }

    /// This provider, sending web calls to `web` (none: each is refused
    /// `web_disabled`), over the same store, host and clock.
    #[must_use]
    pub fn with_web(&self, web: Option<Arc<dyn WebSource>>) -> ClaxProvider {
        ClaxProvider { web, ..self.same() }
    }

    /// This provider, opening browsers from `browser` (none: each
    /// browser call is refused `browser_disabled`), over the same store,
    /// host, web source and clock.
    #[must_use]
    pub fn with_browser(&self, browser: Option<Arc<dyn BrowserSource>>) -> ClaxProvider {
        ClaxProvider {
            browser,
            ..self.same()
        }
    }

    /// This provider, keeping a browser test's screenshots under
    /// `limits` (the host's content limits), over the same store, host,
    /// sources and clock.
    #[must_use]
    pub fn with_content_limits(&self, limits: ContentLimits) -> ClaxProvider {
        ClaxProvider {
            content_limits: limits,
            ..self.same()
        }
    }

    /// A copy of this provider.
    fn same(&self) -> ClaxProvider {
        ClaxProvider {
            store: self.store.clone(),
            host: self.host.clone(),
            web: self.web.clone(),
            browser: self.browser.clone(),
            browser_limits: self.browser_limits,
            content_limits: self.content_limits,
            clock: self.clock.clone(),
            run: self.run.clone(),
        }
    }

    /// This provider, holding `run`'s row: a session of that run is
    /// opened without reading the row from the store again.
    #[must_use]
    pub fn for_run(mut self, run: Run) -> ClaxProvider {
        self.run = Some(run);
        self
    }

    /// The row of run `id`: the one the provider holds, or the store's.
    pub(super) fn row(&self, id: &str) -> clax_core::Result<Run> {
        match &self.run {
            Some(run) if run.id == id => Ok(run.clone()),
            _ => self.store.run(id)?.ok_or(CoreError::NotFound),
        }
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl SurfaceProvider for ClaxProvider {
    fn definition(&self) -> &Definition {
        api::definition()
    }

    /// Every operation but the web's and the browser's, which a run has
    /// by its grants.
    fn standing(&self, op: &str) -> bool {
        api::declared(op, api::OpShape::default())
    }

    /// A web or browser operation the run did not declare is refused in
    /// the words a host with no such source refuses it in.
    fn undeclared(&self, op: &str, args: &[Value]) -> Option<Answer> {
        web::disabled(op, args).or_else(|| browser::disabled(op, args))
    }

    fn for_run(&self, row: &Run, _binding: &BindingAsStored) -> Option<Arc<dyn SurfaceProvider>> {
        Some(Arc::new(
            ClaxProvider {
                run: None,
                ..self.same()
            }
            .for_run(row.clone()),
        ))
    }

    /// The artifact of the binding's scope (`{"artifact": "<ID>"}`).
    fn audit_subject(&self, binding: &Binding) -> Option<AuditIds> {
        let artifact = binding.scope["artifact"].as_str()?;
        Some(AuditIds {
            artifact: Some(artifact.to_string()),
            ..AuditIds::default()
        })
    }

    async fn request(&self, run: &RunInfo) -> Result<Value, HostError> {
        let row = self.row(&run.id).map_err(|e| crate::host::host_error(&e))?;
        let cx = self.host.context(&row).await?;
        Ok(crate::prompt::request_document(row.request, &cx))
    }

    async fn driver_started(&self) -> clax_core::Result<usize> {
        self.host.start_waiting().await
    }

    async fn begin<'s>(&'s self, cx: StepCx<'s>) -> clax_core::Result<Box<dyn StepSession + 's>> {
        let run = self.row(&cx.run.id)?;
        let Tally { base, created, .. } = Tally::of(cx.history);
        let draft = Draft::load(&self.store, &run, base.unwrap_or(run.base_version)).await?;
        Ok(Box::new(ClaxSession {
            provider: self,
            cx,
            audit: bot_ctx(&run.person),
            run,
            created_before: created,
            state: Mutex::new(State {
                draft,
                published: None,
                replied: Vec::new(),
                created: false,
                moved: false,
            }),
            browsing: tokio::sync::Mutex::new(()),
        }))
    }

    async fn working(&self, run: &RunInfo) {
        self.show_working(run).await;
    }

    async fn ending(
        &self,
        cx: StepCx<'_>,
        end: &End,
        result: Option<&Value>,
    ) -> clax_core::Result<Fields> {
        self.run_ending(cx, end, result).await
    }

    async fn ended(
        &self,
        run: &RunInfo,
        _end: &End,
        _result: Option<&Value>,
    ) -> Result<(), HostError> {
        self.run_ended(run).await
    }

    async fn awaiting_ended(&self) -> clax_core::Result<Vec<String>> {
        self.awaiting()
    }
}
