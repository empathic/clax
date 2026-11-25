//! Clax's own surface, which the run loop reaches through the seam
//! ([`crate::surface`]): the facts and rules of artifacts, threads,
//! drafts and the web, which the run loop knows nothing of.
//!
//! - [`provider`]: [`ClaxProvider`], the surface's
//!   [`crate::surface::SurfaceProvider`];
//! - [`session`]: one attempt of a script step, with the step's draft
//!   and what it published, replied and created;
//! - [`rules`]: what a run may reach (its request's threads; one new
//!   artifact, in a brief's run);
//! - [`draft`], [`publish`], [`replies`], [`web`]: the operations, by
//!   what performs them;
//! - [`ending`]: the roster, and what a run's end leaves on its threads
//!   (a notice, or a finished run's summary and sources) and starts
//!   (the follow-ups);
//! - [`tally`]: [`Tally`], what the surface folds from a run's committed
//!   steps ([`crate::surface::StepCx::history`]).
//!
//! The run loop's own folds are [`crate::history`], which reads nothing
//! here.

mod draft;
mod ending;
mod provider;
mod publish;
mod replies;
mod rules;
mod session;
mod tally;
mod web;

pub use provider::ClaxProvider;
#[cfg(test)]
pub(crate) use tally::Tally;
