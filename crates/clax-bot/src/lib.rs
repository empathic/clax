//! ClaxBot's run loop (spec `docs/superpowers/specs/2026-10-21-claxbot-design.md`):
//!
//! - [`api`]: the v0 `clax` script API, defined once, with its TypeScript
//!   declaration, the sandbox's operation names and argument checks;
//! - [`host`]: [`host::BotHost`], what the run loop needs from Clax's
//!   server code, the call journal writes go through, and
//!   [`host::FakeBotHost`]; [`host_conformance`], what every host passes;
//! - [`draft`] and [`check`]: a run's draft and `draft.check`'s static
//!   checks;
//! - [`prompt`]: the system prompt and the request message;
//! - [`policy`]: [`policy::RunPolicy`], what the environment decides
//!   about a run (its limits, what the model is sent of its history),
//!   which the run loop asks instead of holding a limit of its own;
//! - [`run`]: the step state machine, [`run::tick`], with enforcement and
//!   metering; [`script`]: a script step, which knows no surface: it
//!   places, journals and tells each call and hands it to its surface
//!   through the seam;
//! - [`surface`]: the seam the run loop reaches every surface's
//!   operations through, [`surface::SurfaceProvider`] and
//!   [`surface::StepSession`], with [`surface::FakeSurfaceProvider`];
//!   [`provider_conformance`], what every provider passes;
//! - [`registry`]: [`registry::Providers`], the provider of each
//!   surface a driver's runs may be bound to, and what a tick holds of
//!   its run, read from the run's row;
//! - [`clax`]: Clax's own surface as a provider
//!   ([`clax::ClaxProvider`]): the rules of a run's threads and brief,
//!   its draft, publishing, replies and the web operations;
//! - [`bot_surfaces`]: the hub of the host protocol: the surfaces
//!   hosts registered at run time, and the rules by which their calls
//!   go out to them and their answers come back;
//! - [`broker`]: what a provider of a host's surface sends its calls
//!   through ([`broker::CallBroker`]), and its implementation over the
//!   hub;
//! - [`outside`]: the provider of a surface a host registered
//!   ([`outside::OutsideProvider`]), over a broker;
//! - [`history`]: what the run loop reads of a run's committed steps, its
//!   control state and what a model is shown;
//! - [`driver`]: the scheduler over ticks, for the daemon and the Worker;
//! - [`events`]: a run's live events and their replay;
//! - [`web`]: the values of `clax.web.search` and `clax.web.fetch`, and
//!   [`web::WebSource`], where results and pages come from, with
//!   [`web::FakeWebSource`].
//!
//! It builds for native targets and for wasm32 (the hosted Worker); the
//! sandbox and the server are wired in by the crates above it.

// The tests' shared support (`tests/common`), which the unit tests that
// drive whole runs include, names this crate as an outside test does.
#[cfg(test)]
extern crate self as clax_bot;

pub mod api;
pub mod bot_surfaces;
pub mod broker;
pub mod check;
pub mod clax;
pub mod draft;
pub mod driver;
mod ending;
#[cfg(test)]
mod ending_scenarios;
pub mod events;
pub mod finish;
#[cfg(test)]
mod golden;
#[cfg(test)]
mod golden_tests;
pub mod history;
#[cfg(test)]
mod history_fixtures;
pub mod host;
pub mod host_conformance;
pub mod images;
pub mod joins;
pub mod outside;
pub mod policy;
pub mod prompt;
pub mod provider_conformance;
pub mod registry;
pub mod run;
pub mod script;
#[cfg(test)]
mod script_scenarios;
pub mod surface;
pub mod web;

pub use clax_store::Journaled;
/// The surface definitions and the host protocol's messages this crate's
/// API is written in (the hub's, `bot_surfaces`), for the crates above
/// it, which name them through here.
pub use clax_surface;
pub use driver::{Driver, IdleReport};
pub use events::{Event, EventHub, EventId, EventSink, RunEvent, Subscription};
pub use host::{BotHost, FakeBotHost, OpCtx, RequestContext, WritePlan};
pub use policy::{DefaultPolicy, Delivery, RunLimits, RunPolicy};
pub use registry::{Providers, Resolver, RunSurfaces, Unavailable};
pub use run::{
    Deps, Keys, Next, Refusal, RunSettings, SettingsSource, TickOpts, TickOutcome, Wait, refusal,
    tick,
};
pub use web::{FakeWebSource, WebSource};
