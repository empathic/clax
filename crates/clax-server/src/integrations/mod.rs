//! Products built on Clax that this instance also is: each module plays an
//! outside integrator's part, using only the public routes and request
//! types any other product uses, so it reads as that product's code.
//!
//! - [`feedback`]: the instance's people report problems with Clax itself
//!   through a report queue (spec 2026-10-29-feedback-loop-design §10).

pub mod feedback;
