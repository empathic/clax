//! Joins at the moment a run of Clax's own kind is made
//! (`clax_bot::joins`): [`JoinedSurfaces`] answers, for the joins kept on
//! the run's artifact, the binding of each whose registration is
//! connected now. The daemon's hub of registered surfaces is one
//! ([`crate::bot_surfaces::Surfaces`]); a host that keeps no hub (the
//! Worker) has none ([`crate::bot::Bot::joins`] is `None`), and its runs
//! are made with no join whatever the store holds.

use clax_store::bot_joins::Join;
use clax_store::bot_runs::StoredBinding;
use serde_json::Value;

/// Who says what the joins on an artifact bind into a run made there
/// now. Called on a store thread, before the run is written.
pub trait JoinedSurfaces: Send + Sync {
    /// For each of `joins` (of one artifact, each by a surface's
    /// registration), in their order, the binding it gives the run
    /// ([`clax_bot::joins::joined_binding`]), on `scope`
    /// ([`clax_bot::joins::joined_scope`]), when its registration is held
    /// and connected now and gives it an operation; nothing for one that
    /// is not. It waits for no host and changes nothing.
    fn joined(&self, joins: &[Join], scope: &Value) -> Vec<StoredBinding>;
}

#[cfg(not(target_arch = "wasm32"))]
impl JoinedSurfaces for crate::bot_surfaces::Surfaces {
    fn joined(&self, joins: &[Join], scope: &Value) -> Vec<StoredBinding> {
        let limits = self.limits();
        joins
            .iter()
            .filter_map(|join| {
                let held = self.broker().with(|hub, _, now| {
                    hub.connected(&join.registration_id, now)
                        .then(|| hub.snapshot(&join.registration_id))
                        .flatten()
                })?;
                if held.name != join.surface {
                    return None;
                }
                clax_bot::joins::joined_binding(
                    &join.registration_id,
                    &held,
                    join.allow.as_deref(),
                    &limits.definition,
                    scope,
                )
            })
            .collect()
    }
}
