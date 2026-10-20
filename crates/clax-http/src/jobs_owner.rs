//! The queue owner of each job client (spec 2026-10-19-in-process-core-design
//! §7.5, §8.1): the one caller that may write that client's jobs, chosen by
//! the core with no clock. A claim is tied to the claimant's live event
//! stream (`GET /api/stream`) and released when that stream's connection
//! ends, by its token, or when the core closes: it lives in memory only, so
//! a crash, a close or a reopen of the core forgets every claim.

use clax_core::{Event, EventBus};
use clax_realtime::stream::Hub;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The header a write of a client's jobs carries its queue owner's token in.
pub const OWNER_HEADER: &str = "x-clax-jobs-owner";

/// A client's claim: its token and the stream it is tied to.
struct Claim {
    token: String,
    stream: String,
}

/// The queue owner of each job client.
#[derive(Default)]
pub struct JobsOwners {
    claims: Mutex<HashMap<String, Claim>>,
}

/// What a claim answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Claimed {
    /// The claimant owns the client's queue, with this token.
    Owner(String),
    /// Another claimant's stream holds it.
    Held,
    /// The stream named is not one the hub holds a connection of.
    NoSuchStream,
}

/// A claim's token: 32 hex characters from the operating system's random
/// source.
fn new_token() -> String {
    clax_core::secret::random_bytes::<16>()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl JobsOwners {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Claim>> {
        // A claim is only ever replaced whole: a poisoned lock still holds them.
        self.claims.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Claims `client`'s queue for the holder of event stream `stream`.
    /// [`Claimed::Held`] while another claim's stream has a connection
    /// open, and that connection is nudged ([`Hub::nudge`]) so a holder
    /// whose reader is gone is noticed at once; a claim by the stream that
    /// holds it answers its token again. On [`Claimed::Owner`] it publishes
    /// `Event::JobsOwner { held: true }` and spawns a task that waits for
    /// the stream's connection to end ([`Hub::closed`]) and then releases
    /// the claim, publishing `Event::JobsOwner { held: false }`.
    pub fn claim(
        self: &Arc<Self>,
        hub: &Hub,
        events: &EventBus,
        client: &str,
        stream: &str,
    ) -> Claimed {
        if !hub.is_attached(stream) {
            return Claimed::NoSuchStream;
        }
        let token = {
            let mut claims = self.lock();
            if let Some(c) = claims.get(client) {
                if c.stream == stream {
                    return Claimed::Owner(c.token.clone());
                }
                if hub.is_attached(&c.stream) {
                    let held = c.stream.clone();
                    drop(claims);
                    hub.nudge(&held);
                    return Claimed::Held;
                }
            }
            let token = new_token();
            claims.insert(
                client.to_string(),
                Claim {
                    token: token.clone(),
                    stream: stream.to_string(),
                },
            );
            token
        };
        events.publish(Event::JobsOwner {
            client: client.to_string(),
            held: true,
        });
        let closed = hub.closed(stream);
        let owners = self.clone();
        let events = events.clone();
        let (client, held) = (client.to_string(), token.clone());
        clax_rt::spawn(async move {
            closed.await;
            if owners.release(&client, &held) {
                events.publish(Event::JobsOwner {
                    client,
                    held: false,
                });
            }
        });
        Claimed::Owner(token)
    }

    /// Releases `client`'s claim when `token` holds it; whether it did.
    /// The caller publishes `Event::JobsOwner { held: false }`.
    pub fn release(&self, client: &str, token: &str) -> bool {
        let mut claims = self.lock();
        let holds = claims.get(client).is_some_and(|c| {
            clax_core::secret::constant_time_eq(c.token.as_bytes(), token.as_bytes())
        });
        if holds {
            claims.remove(client);
        }
        holds
    }

    /// Releases the claim `token` holds, whichever client's it is; that
    /// client when there was one.
    pub fn release_token(&self, token: &str) -> Option<String> {
        let mut claims = self.lock();
        let client = claims
            .iter()
            .find(|(_, c)| {
                clax_core::secret::constant_time_eq(c.token.as_bytes(), token.as_bytes())
            })
            .map(|(k, _)| k.clone())?;
        claims.remove(&client);
        Some(client)
    }

    /// Whether `token` holds `client`'s queue.
    pub fn holds(&self, client: &str, token: &str) -> bool {
        self.lock().get(client).is_some_and(|c| {
            clax_core::secret::constant_time_eq(c.token.as_bytes(), token.as_bytes())
        })
    }
}
