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
//! - [`run`]: the step state machine, [`run::tick`], with enforcement and
//!   metering; [`script`]: a script step;
//! - [`driver`]: the scheduler over ticks, for the daemon and the Worker;
//! - [`events`]: a run's live events and their replay.
//!
//! It builds for native targets and for wasm32 (the hosted Worker); the
//! sandbox and the server are wired in by the crates above it.

pub mod api;
pub mod check;
pub mod draft;
pub mod driver;
pub mod events;
pub mod host;
pub mod host_conformance;
pub mod prompt;
pub mod run;
pub mod script;

pub use clax_store::Journaled;
pub use driver::{Driver, IdleReport};
pub use events::{Event, EventHub, EventId, EventSink, RunEvent, Subscription};
pub use host::{BotHost, FakeBotHost, OpCtx, RequestContext, WritePlan};
pub use run::{
    Deps, Keys, Next, Refusal, RunSettings, SettingsSource, TickOpts, TickOutcome, Wait, refusal,
    tick,
};
