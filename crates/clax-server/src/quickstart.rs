//! The Clax Quickstart in each hosted gallery (spec
//! 2026-10-26-quickstart-onboarding-design §9): on a hosted site a person
//! at their first sign-in or invite accept and again at every later
//! sign-in (GitHub or a sign-in link). A local core seeds nothing; at start
//! it only updates a quickstart its owner's gallery already holds. The
//! store's seed and update
//! ([`clax_store::Store::seed_quickstart`],
//! [`clax_store::Store::update_quickstart`]) run for a person with the
//! release the web UI carries ([`AppState::quickstart`]); a state with no
//! release does nothing. A seed is once per person and only for a gallery
//! that never held an artifact, so repeating it at each sign-in retries one
//! that failed and is otherwise a no-op; an update moves an untouched copy
//! to the current release. Failures are logged and never fail the caller,
//! and a version made is told to the event streams as a publish tells it.

use clax_core::Event;
use clax_core::audit::AuditCtx;
use clax_core::model::{Artifact, Version};
use clax_http::state::AppState;
use clax_store::quickstart::{Seeded, Updated};

/// Seeds the quickstart into the gallery of `person` (`None`: the owner).
/// Nothing happens when the state carries no release. A failure is logged.
pub(crate) async fn seed(s: &AppState, person: Option<&str>) {
    let Some(release) = s.quickstart.clone() else {
        return;
    };
    let who = person.map(str::to_string);
    let made = s
        .store_call_async(move |st| async move {
            st.seed_quickstart(&AuditCtx::DAEMON, who.as_deref(), &release)
                .await
        })
        .await;
    match made {
        Ok(Seeded::Made { artifact, version }) => {
            tracing::info!(artifact = %artifact.id, "seeded the quickstart");
            told(s, &artifact, &version);
        }
        Ok(Seeded::Already | Seeded::Skipped) => {}
        Err(e) => tracing::warn!(error = ?e, "could not seed the quickstart"),
    }
}

/// Moves the untouched quickstart of `person` (`None`: the owner) to the
/// release the state carries. Nothing happens when the state carries none.
/// A failure is logged.
pub(crate) async fn update(s: &AppState, person: Option<&str>) {
    let Some(release) = s.quickstart.clone() else {
        return;
    };
    let who = person.map(str::to_string);
    let moved = s
        .store_call_async(move |st| async move {
            st.update_quickstart(&AuditCtx::DAEMON, who.as_deref(), &release)
                .await
        })
        .await;
    match moved {
        Ok(Updated::Made { artifact, version }) => {
            tracing::info!(artifact = %artifact.id, n = version.n, "updated the quickstart");
            told(s, &artifact, &version);
        }
        Ok(Updated::Current | Updated::Stopped) => {}
        Err(e) => tracing::warn!(error = ?e, "could not update the quickstart"),
    }
}

/// A person's sign-in: [`seed`], then [`update`]. Not for the owner or a
/// guest.
pub(crate) async fn sign_in(s: &AppState, person: &str) {
    seed(s, Some(person)).await;
    update(s, Some(person)).await;
}

fn told(s: &AppState, artifact: &Artifact, version: &Version) {
    s.events.publish(Event::Version {
        artifact_id: artifact.id.clone(),
        n: version.n,
        by_page: false,
        title: Some(artifact.title.clone()),
        at: Some(version.created_at.clone()),
    });
}
