//! ClaxBot runs end to end: a scripted Messages API on 127.0.0.1, the
//! QuickJS sandbox, a fake host and a store on a scratch home with an
//! injected clock. No test reaches the network or sleeps for a condition.

mod common;

mod cancel;
mod driver;
mod limits;
mod money;
mod replay;
mod resume;
mod rules;
mod runs;
