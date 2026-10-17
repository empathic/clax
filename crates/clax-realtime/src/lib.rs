//! The daemon's in-memory delivery: the event-stream hub (`stream`), rooms
//! (`room`), presence, the Codex push (`push`), agent working records
//! (`working`), feedback waiters and fan-out (`feedback`), question waiters
//! (`questions`), the live-page IDs (`live`) and the wait of a call-ID report
//! for its tool call (`call_ids`). It holds no HTTP types and no `AppState`:
//! the routes and their views reach it through the daemon's state.

pub mod call_ids;
pub mod feedback;
pub mod live;
pub mod presence;
pub mod push;
pub mod questions;
pub mod room;
pub mod stream;
pub mod working;
