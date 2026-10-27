//! The Clax Quickstart at the start of a local core (spec
//! 2026-10-26-quickstart-onboarding-design §9): the store's seed and update
//! ([`clax_store::Store::seed_quickstart`],
//! [`clax_store::Store::update_quickstart`]) run for a person with the
//! release the web UI carries ([`AppState::quickstart`]), and a version they
//! make is told to the event streams as a publish tells it.

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

fn told(s: &AppState, artifact: &Artifact, version: &Version) {
    s.events.publish(Event::Version {
        artifact_id: artifact.id.clone(),
        n: version.n,
        by_page: false,
        title: Some(artifact.title.clone()),
        at: Some(version.created_at.clone()),
    });
}
