//! Commits pushed to an artifact's repository, as versions: the store
//! records them (`Store::reconcile_pushes_async`), on the push itself
//! ([`reconcile`], which the Worker calls once a push to the stand-in is
//! taken, or for an Artifacts `pushed` event) and before every publish;
//! [`listen`] tells the streams of each one as of a published version,
//! so viewers see a pushed version arrive as they see a publish.

use clax_core::ArtifactId;
use clax_core::audit::AuditCtx;
use clax_core::events::Event;
use clax_store::pushes::Pusher;

use crate::error::ApiError;
use crate::state::AppState;

/// Publishes `Event::Version` for every version the store records from a
/// pushed commit, and `Event::Permissions` after it when its declaration
/// changed the artifact's capability approval, so open views refetch what
/// is in force; and tells the store whether it is a hosted daemon's
/// (`Store::set_hosted`).
pub fn listen(s: &AppState) {
    // Who administers an artifact for a pushed declaration.
    s.store.set_hosted(s.hosted.is_some());
    let events = s.events.clone();
    s.store.set_pushed_listener(move |a, v, approval_changed| {
        events.publish(Event::Version {
            artifact_id: a.id.clone(),
            n: v.n,
            by_page: false,
            title: Some(a.title.clone()),
            at: Some(v.created_at.clone()),
        });
        if approval_changed {
            events.publish(Event::Permissions {
                artifact_id: a.id.clone(),
            });
        }
    });
}

/// Records the commits pushed to artifact `id` as versions published by
/// `pusher`; answers the numbers recorded, oldest first.
pub async fn reconcile(s: &AppState, id: ArtifactId, pusher: Pusher) -> Result<Vec<u32>, ApiError> {
    s.store_call_async(move |st| async move {
        let recorded = st
            .reconcile_pushes_async(&AuditCtx::DAEMON, &id, &pusher)
            .await?;
        Ok(recorded.into_iter().map(|(_, v)| v.n).collect())
    })
    .await
}
