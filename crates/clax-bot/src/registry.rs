//! The providers of the surfaces a driver's runs may be bound to, and
//! what a tick holds of one run: [`Providers`], by the surface's name,
//! and [`RunSurfaces`], the run as the run loop knows it
//! ([`RunInfo`]) with the provider of each of its bindings.
//!
//! Whoever builds a [`Deps`](crate::run::Deps) registers a provider for
//! each surface it serves ([`Providers::register`]);
//! [`Deps::new`](crate::run::Deps::new) registers Clax's own
//! ([`ClaxProvider`]) over the host it is given, which is the one place
//! the run loop's side names it. A tick reads its run's row once, with
//! what the run was made as
//! ([`RunAsMade`], `Store::run_with_steps_as_made`), and from the two
//! ([`Providers::bind`]):
//!
//! - the run's kind (`bot_runs.kind`) and its request
//!   (`bot_runs.request_json`), as stored;
//! - its bindings (`bot_runs.bindings_json`, version 1), each made a
//!   [`Binding`] with its surface's definition and standing operations,
//!   which its provider says ([`SurfaceProvider::definition`],
//!   [`SurfaceProvider::standing`]);
//! - the provider of each binding: the one registered under the
//!   binding's surface, or the one it answers for a tick of this run
//!   ([`SurfaceProvider::for_run`]); for a surface with none
//!   registered, the one the registry's [`Resolver`] answers for the
//!   binding;
//! - what the run's audit records are filed on
//!   ([`RunSurfaces::subject`]);
//! - the operations the run's scripts are given
//!   ([`RunSurfaces::operations`]).
//!
//! A binding whose surface has no provider registered, and which the
//! resolver does not serve, stays bound, to [`Unavailable`]: every
//! call a script makes to it is answered `unavailable`, and it takes
//! no part in the run's end.
//!
//! # Surfaces registered while a driver runs
//!
//! What is registered by name is fixed when the deps are made: Clax's
//! own surface, and whatever a test registers. A host registers its
//! surface with a running daemon, under a name no one knew when the
//! deps were made, and many registrations may share one name. Such a
//! surface is not registered here. The daemon gives the registry one
//! [`Resolver`] ([`Providers::with_resolver`]), which is asked for
//! each stored binding whose surface has no provider registered and
//! answers the provider of that binding: for a binding that names a
//! registration, [`crate::outside::Registrations`] answers an
//! [`crate::outside::OutsideProvider`] over the daemon's broker.
//!
//! A provider the resolver answers lives for one tick. A driver that
//! starts asks nothing of it ([`SurfaceProvider::driver_started`],
//! [`SurfaceProvider::awaiting_ended`] reach the providers registered
//! by name), and it needs neither: an outside provider's `ended`
//! records the run's end with the broker, whose store keeps it and
//! whose hub delivers it to the host until the host acknowledges, so
//! nothing is owed a repeat when a driver starts.

use crate::clax::ClaxProvider;
use crate::host::BotHost;
use crate::script::Surface;
use crate::surface::{
    Answer, Binding, BindingAsStored, Call, End, Fields, RunInfo, StepCx, StepEnd, StepParts,
    StepSession, SurfaceProvider,
};
use crate::web::WebSource;
use clax_bot_sandbox::HostError;
use clax_bot_sandbox::MaybeSendSync;
use clax_core::audit::AuditIds;
use clax_core::working::Clock;
use clax_core::{CoreError, Result};
use clax_store::Store;
use clax_store::bot_runs::{BindingsError, CLAX_SURFACE, Run, RunAsMade};
use clax_surface::{Author, Definition, Limits, Reserved};
use serde_json::Value;
use std::sync::{Arc, LazyLock};

/// Answers the provider of a binding whose surface has no provider
/// registered by name: what a daemon gives the registry once, for the
/// surfaces hosts register while it runs (the module documentation).
pub trait Resolver: MaybeSendSync {
    /// The provider of `binding`, a binding of the run whose row is
    /// `row` and whose kind is `kind` (`<surface>.<kind>`), for this
    /// tick. `None` for a binding it does not serve, which is then
    /// served by [`Unavailable`]. It reads no store and waits for
    /// nothing.
    fn resolve(
        &self,
        row: &Run,
        kind: &str,
        binding: &BindingAsStored,
    ) -> Option<Arc<dyn SurfaceProvider>>;
}

/// The provider of each surface a driver's runs may be bound to, by the
/// surface's name, in the order they were registered, and the resolver
/// asked for a binding of any other surface.
#[derive(Clone, Default)]
pub struct Providers {
    by_surface: Vec<(String, Arc<dyn SurfaceProvider>)>,
    resolver: Option<Arc<dyn Resolver>>,
    /// Clax's own provider, as [`Providers::register_clax`] registered
    /// it and while no other is registered for its surface: kept so that
    /// `set_clax_web` can give it another web source.
    clax: Option<Arc<ClaxProvider>>,
}

impl std::fmt::Debug for Providers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.by_surface.iter().map(|(surface, _)| surface))
            .finish()
    }
}

impl Providers {
    /// No provider.
    #[must_use]
    pub fn new() -> Providers {
        Providers::default()
    }

    /// Registers `provider` for the bindings of `surface`, in place of
    /// the one registered for it before, which is returned.
    pub fn register(
        &mut self,
        surface: impl Into<String>,
        provider: Arc<dyn SurfaceProvider>,
    ) -> Option<Arc<dyn SurfaceProvider>> {
        let surface = surface.into();
        if surface == CLAX_SURFACE {
            self.clax = None;
        }
        match self.by_surface.iter_mut().find(|(s, _)| *s == surface) {
            Some((_, held)) => Some(std::mem::replace(held, provider)),
            None => {
                self.by_surface.push((surface, provider));
                None
            }
        }
    }

    /// These providers, with `provider` registered for `surface`
    /// ([`Providers::register`]).
    #[must_use]
    pub fn with(
        mut self,
        surface: impl Into<String>,
        provider: Arc<dyn SurfaceProvider>,
    ) -> Providers {
        self.register(surface, provider);
        self
    }

    /// These providers, with `resolver` asked for each binding whose
    /// surface has no provider registered ([`Resolver`]), in place of
    /// the one set before.
    #[must_use]
    pub fn with_resolver(mut self, resolver: Arc<dyn Resolver>) -> Providers {
        self.resolver = Some(resolver);
        self
    }

    /// Sets the resolver ([`Providers::with_resolver`]).
    pub fn set_resolver(&mut self, resolver: Arc<dyn Resolver>) {
        self.resolver = Some(resolver);
    }

    /// The provider registered for `surface`.
    #[must_use]
    pub fn get(&self, surface: &str) -> Option<&Arc<dyn SurfaceProvider>> {
        self.by_surface
            .iter()
            .find(|(s, _)| s == surface)
            .map(|(_, provider)| provider)
    }

    /// Every provider registered, with its surface's name, in the order
    /// they were registered.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Arc<dyn SurfaceProvider>)> {
        self.by_surface
            .iter()
            .map(|(surface, provider)| (surface.as_str(), provider))
    }

    /// The provider that serves a binding of `surface`: the one
    /// registered for it, or [`Unavailable`].
    #[must_use]
    pub fn of(&self, surface: &str) -> Arc<dyn SurfaceProvider> {
        match self.get(surface) {
            Some(provider) => provider.clone(),
            None => Arc::new(Unavailable::new(surface)),
        }
    }

    /// Registers Clax's own surface ([`ClaxProvider`]) over `store` and
    /// `host`, sending web calls to `web`, with `clock` for the credits
    /// week.
    pub(crate) fn register_clax(
        &mut self,
        store: Arc<Store>,
        host: Arc<dyn BotHost>,
        web: Option<Arc<dyn WebSource>>,
        clock: Arc<dyn Clock>,
    ) {
        let clax = Arc::new(ClaxProvider::new(store, host, web, clock));
        self.register(CLAX_SURFACE, clax.clone());
        self.clax = Some(clax);
    }

    /// Registers Clax's own surface again as [`Providers::register_clax`]
    /// last registered it, but sending its web calls to `web` (with
    /// none, each is refused `web_disabled`). Nothing changes when no
    /// provider of Clax's is registered so: none was, or another
    /// provider was registered for its surface since.
    pub(crate) fn set_clax_web(&mut self, web: Option<Arc<dyn WebSource>>) {
        if let Some(clax) = self.clax.take() {
            let clax = Arc::new(clax.with_web(web));
            self.register(CLAX_SURFACE, clax.clone());
            self.clax = Some(clax);
        }
    }

    /// The run whose row is `row` and which was made as `made`, as the
    /// run loop knows it, with the provider of each of its bindings:
    /// see the module documentation. Nothing is read from the store;
    /// of `row` the run loop takes the run's ID, person and model, and
    /// hands the row to each provider that wants it
    /// ([`SurfaceProvider::for_run`]).
    ///
    /// # Errors
    /// What this build cannot read of the run as it was made, each a
    /// code a run ends `failed` with ([`Providers::bind_or_unreadable`]):
    /// [`CODE_BINDINGS_UNSUPPORTED`] for bindings of a version a later
    /// build wrote, [`CODE_BINDINGS_INVALID`] for text that is not
    /// bindings or a binding whose kept settings this build does not read
    /// (a setting a later build keeps), [`CODE_REQUEST_INVALID`] for a
    /// request that is not JSON.
    pub fn bind(&self, row: &Run, made: &RunAsMade) -> Result<RunSurfaces> {
        let stored = made.bindings().map_err(|e| {
            let code = match e {
                BindingsError::Unsupported { .. } => CODE_BINDINGS_UNSUPPORTED,
                BindingsError::Invalid(_) => CODE_BINDINGS_INVALID,
            };
            CoreError::invalid(code, format!("the bindings of run {}: {e}", row.id))
        })?;
        let request: Value = serde_json::from_str(&made.request_json).map_err(|e| {
            CoreError::invalid(
                CODE_REQUEST_INVALID,
                format!("the request of run {} is not JSON: {e}", row.id),
            )
        })?;
        let mut bindings = Vec::with_capacity(stored.all().len());
        let mut providers = Vec::with_capacity(stored.all().len());
        let mut operations = Vec::new();
        for binding in stored.all() {
            let kept = BindingAsStored::of(binding);
            // Settings this build does not read are no settings to run
            // under: the run's limits would be lost with them.
            if let Err(e) = crate::outside::kept_settings(&kept) {
                return Err(CoreError::invalid(
                    CODE_BINDINGS_INVALID,
                    format!(
                        "the bindings of run {}: the settings its binding of {} keeps are not \
                         settings this build reads: {e}",
                        row.id, kept.surface
                    ),
                ));
            }
            let provider = match self.get(&kept.surface) {
                Some(registered) => registered
                    .for_run(row, &kept)
                    .unwrap_or_else(|| registered.clone()),
                None => self
                    .resolver
                    .as_ref()
                    .and_then(|resolver| resolver.resolve(row, &made.kind, &kept))
                    .unwrap_or_else(|| Arc::new(Unavailable::new(&kept.surface))),
            };
            let bound = Binding::as_stored(binding, &*provider);
            // What a script of the run can name: every operation of the
            // surface's definition, or, of a surface whose definition is
            // not known here, those the row keeps for the binding.
            if provider.unavailable().is_some() {
                operations.extend(bound.operations.iter().cloned());
            } else {
                let defined = provider.definition().operations();
                operations.extend(defined.map(|o| o.name.to_string()));
            }
            bindings.push(bound);
            providers.push(provider);
        }
        let info = RunInfo {
            id: row.id.clone(),
            person: row.person.clone(),
            model: row.model.clone(),
            kind: made.kind.clone(),
            request,
            bindings,
        };
        let kind = match info
            .bindings
            .iter()
            .position(|b| b.surface == info.kind_surface())
        {
            Some(at) => providers[at].clone(),
            None => self.of(info.kind_surface()),
        };
        let mut bound = RunSurfaces {
            info,
            providers,
            kind,
            operations,
            unreadable: None,
            subject: AuditIds::default(),
        };
        let subject = bound
            .surfaces()
            .iter()
            .find_map(|surface| surface.provider.audit_subject(surface.binding));
        bound.subject = subject.unwrap_or_default();
        Ok(bound)
    }
}

impl Providers {
    /// [`Providers::bind`], or, for a run whose bindings or request
    /// this build cannot read, the run with no binding at all and the
    /// reason ([`RunSurfaces::unreadable`]): what a tick holds of such
    /// a run, so that it can end it `failed` (a cancel is honoured
    /// first) where it would otherwise fail every tick, uncounted, for
    /// good. No provider is told of that end: none could be found.
    ///
    /// # Errors
    /// Any other error of [`Providers::bind`]'s.
    pub fn bind_or_unreadable(&self, row: &Run, made: &RunAsMade) -> Result<RunSurfaces> {
        let (code, why) = match self.bind(row, made) {
            Err(CoreError::Invalid { code, message })
                if [
                    CODE_BINDINGS_UNSUPPORTED,
                    CODE_BINDINGS_INVALID,
                    CODE_REQUEST_INVALID,
                ]
                .contains(&code) =>
            {
                (code, message)
            }
            other => return other,
        };
        Ok(RunSurfaces {
            info: RunInfo {
                id: row.id.clone(),
                person: row.person.clone(),
                model: row.model.clone(),
                kind: made.kind.clone(),
                request: Value::Null,
                bindings: Vec::new(),
            },
            providers: Vec::new(),
            kind: Arc::new(Unavailable::new(made.kind.split('.').next().unwrap_or(""))),
            operations: Vec::new(),
            unreadable: Some((code, why)),
            subject: AuditIds::default(),
        })
    }
}

/// A run as the run loop knows it ([`RunInfo`]), with the provider of
/// each of its bindings: what a tick holds of its run
/// ([`Providers::bind`]).
pub struct RunSurfaces {
    /// The run: its kind, its request and its bindings, as stored.
    pub info: RunInfo,
    /// The provider of each of `info.bindings`, in their order.
    providers: Vec<Arc<dyn SurfaceProvider>>,
    /// The provider of the surface that declared the run's kind.
    kind: Arc<dyn SurfaceProvider>,
    /// [`RunSurfaces::operations`].
    operations: Vec<String>,
    /// [`RunSurfaces::unreadable`].
    unreadable: Option<(&'static str, String)>,
    /// What the run's audit records are filed on: what the first of its
    /// bindings' providers to name a subject answered
    /// ([`SurfaceProvider::audit_subject`]), asked in the order of
    /// [`RunSurfaces::surfaces`]. For a run with a `clax` binding that
    /// is the binding's artifact. No binding naming one, the records
    /// carry no object's ID and are filed on the person alone, whom
    /// their actor names.
    pub subject: AuditIds,
}

impl std::fmt::Debug for RunSurfaces {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunSurfaces")
            .field("info", &self.info)
            .field("subject", &self.subject)
            .finish_non_exhaustive()
    }
}

impl RunSurfaces {
    /// Each binding of the run with its provider: the surface that
    /// declared the run's kind first, then the run's other bindings in
    /// the order they were bound. It is the order a step's sessions are
    /// opened, finished and joined in, and the order the bindings are
    /// told of the run's end in.
    #[must_use]
    pub(crate) fn surfaces(&self) -> Vec<Surface<'_>> {
        let mut surfaces: Vec<Surface<'_>> = self
            .info
            .bindings
            .iter()
            .zip(&self.providers)
            .map(|(binding, provider)| Surface {
                binding,
                provider: &**provider,
            })
            .collect();
        let kind = self.info.kind_surface();
        surfaces.sort_by_key(|surface| surface.binding.surface != kind);
        surfaces
    }

    /// Why this build cannot drive the run, for a run whose bindings or
    /// request it could not read ([`Providers::bind_or_unreadable`]):
    /// the code the run ends `failed` with, and what was wrong, in
    /// words. `None` for a run that was read.
    #[must_use]
    pub fn unreadable(&self) -> Option<(&'static str, &str)> {
        self.unreadable
            .as_ref()
            .map(|(code, why)| (*code, why.as_str()))
    }

    /// The operations a script of the run can name, each as it is
    /// called after `clax.` (`files.read`, `shell.windows`): what the
    /// sandbox builds the script's `clax` from
    /// (`clax_bot_sandbox::Script::with_ops`). For each binding, in the
    /// order they were bound, every operation of its surface's
    /// definition, in the definition's order; for a binding whose
    /// provider takes no call and knows no definition, the operations
    /// the row keeps for it. It is every operation of the definition
    /// and not the run's own set ([`Binding::operations`]), so that a
    /// call to one the run lacks reaches the run loop and is refused in
    /// its surface's words (`web_disabled`, for `clax`). For a run
    /// whose one binding is `clax`'s this is
    /// [`crate::api::op_names`], the list a sandbox is built with.
    #[must_use]
    pub fn operations(&self) -> &[String] {
        &self.operations
    }

    /// The definition of each of the run's surfaces, with its binding:
    /// the surface that declared the run's kind first, then the run's
    /// other bindings in the order they were bound. What one
    /// declaration of everything a run's scripts can call is written
    /// from.
    #[must_use]
    pub fn definitions(&self) -> Vec<(&Binding, &Definition)> {
        self.surfaces()
            .into_iter()
            .map(|surface| (surface.binding, surface.provider.definition()))
            .collect()
    }

    /// The provider of the surface that declared the run's kind: the
    /// run's binding of that surface, or, for a run that is not bound
    /// to it, what [`Providers::of`] answers for its name.
    #[must_use]
    pub fn kind_provider(&self) -> &dyn SurfaceProvider {
        &*self.kind
    }
}

/// The reason a run ends `failed` for when its bindings are of a
/// version this build does not read: a later build made the run, and no
/// retry changes that.
pub const CODE_BINDINGS_UNSUPPORTED: &str = "bindings_unsupported";
/// The reason a run ends `failed` for when its stored bindings are not
/// bindings.
pub const CODE_BINDINGS_INVALID: &str = "bindings_invalid";
/// The reason a run ends `failed` for when its stored request is not
/// JSON.
pub const CODE_REQUEST_INVALID: &str = "request_invalid";

/// The reason a run ends `failed` for when the operations of its
/// surfaces, together, are a list no sandbox can build a script's
/// `clax` from (`clax_bot_sandbox::validate_ops`: two surfaces with
/// one operation's name, an operation named as another's namespace,
/// too many). No script could run, so the model is shown no step of
/// it.
pub const CODE_OPS_INVALID: &str = "surface_ops_invalid";

/// The reason a run ends `failed` for when the prompt file it was
/// started with, which its binding names by hash
/// ([`BindingAsStored::prompt`]), is no longer kept: the run is sent no
/// other prompt in its place.
pub const CODE_PROMPT_GONE: &str = "prompt_gone";

/// The code of the error a call to a surface with no provider is
/// answered ([`Unavailable`]): no provider accepted the call, and
/// nothing happened.
pub const CODE_UNAVAILABLE: &str = "unavailable";

/// A definition with no operation and no kind: what [`Unavailable`]
/// knows of its surface, whose own definition a stored binding of
/// version 1 does not keep.
pub(crate) static NOTHING: LazyLock<Definition> = LazyLock::new(|| {
    let reserved = Reserved::of(&[crate::api::definition()]);
    let host = Author::Host {
        reserved: &reserved,
    };
    clax_surface::parse("declare const away: {};", host, &Limits::default())
        .unwrap_or_else(|e| panic!("the definition of a surface with no provider: {e}"))
});

/// The provider of a binding whose surface has none registered: a host
/// that is away. The binding stays bound. Every call a script makes to
/// the surface is answered `unavailable` ([`CODE_UNAVAILABLE`]) by the
/// run loop, before its arguments are checked
/// ([`SurfaceProvider::unavailable`]), since the surface's definition
/// is not known here: the binding has no operation. The provider shows
/// nothing of a run at work, answers no field of a run's end, is owed
/// no `ended`, names no subject and cannot resolve a request.
#[derive(Clone, Debug)]
pub struct Unavailable {
    surface: String,
}

impl Unavailable {
    /// The provider of the bindings of `surface`, which has none.
    #[must_use]
    pub fn new(surface: impl Into<String>) -> Unavailable {
        Unavailable {
            surface: surface.into(),
        }
    }

    fn error(&self) -> HostError {
        HostError::new(
            CODE_UNAVAILABLE,
            format!(
                "the surface {} has no provider here now: nothing happened",
                self.surface
            ),
        )
        .retryable()
    }
}

/// A session of [`Unavailable`]: it answers every call `unavailable`
/// and adds nothing to its step.
struct Away(HostError);

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl StepSession for Away {
    async fn call(&self, _call: Call<'_>) -> Answer {
        Answer::refused(self.0.clone())
    }

    fn finish(self: Box<Self>, _end: StepEnd) -> StepParts {
        StepParts::default()
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl SurfaceProvider for Unavailable {
    fn definition(&self) -> &Definition {
        &NOTHING
    }

    fn unavailable(&self) -> Option<HostError> {
        Some(self.error())
    }

    async fn request(&self, _run: &RunInfo) -> std::result::Result<Value, HostError> {
        Err(self.error())
    }

    async fn begin<'s>(&'s self, _cx: StepCx<'s>) -> Result<Box<dyn StepSession + 's>> {
        Ok(Box::new(Away(self.error())))
    }

    async fn ending(&self, _cx: StepCx<'_>, _end: &End, _result: Option<&Value>) -> Result<Fields> {
        Ok(Fields::new())
    }

    async fn ended(
        &self,
        _run: &RunInfo,
        _end: &End,
        _result: Option<&Value>,
    ) -> std::result::Result<(), HostError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The code of `source` before its tests, without its comment
    /// lines.
    fn code_of(source: &str) -> String {
        source
            .split("#[cfg(test)]\nmod tests")
            .next()
            .expect("the code before the tests")
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The run loop reaches Clax's own surface through the registry and
    /// the seam alone: the tick, the driver, the endings and the script
    /// step import nothing of its provider, read none of its columns of
    /// a run's row, call no method of its host, and name the host in the
    /// two lines listed alone, by which `Deps::new` hands the host it is
    /// given to the registry. That a run runs with no
    /// provider of Clax's at all is held by `tests/generic.rs`, whose
    /// registry holds the fake alone.
    #[test]
    fn the_run_loop_names_nothing_of_claxs_but_what_is_listed() {
        let files = [
            ("run.rs", include_str!("run.rs")),
            ("driver.rs", include_str!("driver.rs")),
            ("ending.rs", include_str!("ending.rs")),
            ("script.rs", include_str!("script.rs")),
        ];
        // The lines that name the host: `Deps::new` is given it and
        // hands it to the registry, which makes Clax's provider over it.
        // The deps keep no host.
        let host_lines = [
            ("run.rs", "host: Arc<dyn BotHost>,"),
            (
                "run.rs",
                "providers.register_clax(store.clone(), host, None, clock.clone());",
            ),
        ];
        let mut seen = Vec::new();
        for (file, source) in files {
            let code = code_of(source);
            for named in [
                "crate::clax",
                "ClaxProvider",
                "artifact_id",
                "base_version",
                "thread_ids",
                "RequestKind",
                "run.request",
                "run.web",
                "deps.web",
                ".context(",
                ".start_waiting(",
                ".plan_",
            ] {
                assert!(!code.contains(named), "{file} names `{named}`");
            }
            for line in code.lines().map(str::trim) {
                // The deps' host, by its type or as a field or a
                // value handed on; not the script step's own
                // `StepHost`, nor a function of `crate::host`.
                let names_host = line.contains("BotHost")
                    || line.contains(".host")
                    || line.contains("host.clone()")
                    || line == "host,"
                    || line.contains("register_clax(");
                if names_host && !line.starts_with("use crate::host::") {
                    seen.push((file, line.to_string()));
                }
            }
        }
        let listed: Vec<(&str, String)> = host_lines
            .iter()
            .map(|(file, line)| (*file, (*line).to_string()))
            .collect();
        assert_eq!(
            seen, listed,
            "the lines of the run loop that name Clax's host"
        );
    }

    /// A provider is registered for a surface's name, in place of the
    /// one before it, and a surface with none is served by
    /// [`Unavailable`].
    #[test]
    fn a_surface_is_served_by_the_provider_registered_last_or_by_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&clax_core::Home::at(dir.path().join("ax"))).unwrap());
        let fake = || -> Arc<dyn SurfaceProvider> {
            Arc::new(crate::surface::FakeSurfaceProvider::new(store.clone()))
        };
        let (first, second) = (fake(), fake());
        let mut providers = Providers::new();
        assert!(providers.register("fake", first.clone()).is_none());
        assert!(providers.register("other", first.clone()).is_none());
        let replaced = providers
            .register("fake", second.clone())
            .expect("the first");
        assert!(Arc::ptr_eq(&replaced, &first));
        assert!(Arc::ptr_eq(providers.get("fake").unwrap(), &second));
        assert!(Arc::ptr_eq(&providers.of("other"), &first));
        let names: Vec<&str> = providers.iter().map(|(surface, _)| surface).collect();
        assert_eq!(names, ["fake", "other"]);
        assert!(providers.get("shell").is_none());
        let away = providers.of("shell");
        let error = away.unavailable().expect("no provider");
        assert_eq!(error.code, CODE_UNAVAILABLE);
        assert!(error.retryable);
        assert_eq!(away.definition().operations().len(), 0);
        assert!(away.definition().kinds().is_empty());
        assert!(first.unavailable().is_none());
    }
}
