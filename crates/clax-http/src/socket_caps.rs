//! Caps on the long-lived connections one client holds on a hosted daemon:
//! `/api/stream` streams (the SSE body natively, the hibernatable
//! WebSocket on a Worker), live-layers sockets and room sockets, together.
//! Each is counted against the client's address ([`crate::hosted::Hosted::client`]:
//! `CF-Connecting-IP` behind Cloudflare, else the peer; an IPv6 address by
//! its /64) and, with a browser session, against that session too, from
//! its admission until the connection is dropped ([`SocketSlot`]). A
//! request past either cap is refused before its upgrade, 429
//! `too_many_streams`.
//!
//! A local daemon (no `[hosted]`) caps nothing: its clients are the owner's
//! machine and its LAN, and the realtime load gate drives thousands of
//! streams from one address (`scripts/perf-clients.py`).
//!
//! A host that lost its connections' state (a Durable Object woken from
//! hibernation) counts each socket it reopens again without refusing it
//! ([`SocketCaps::hold`]), from the address and session its credentials
//! kept ([`crate::identity::SocketCredentials`]): those sockets were
//! admitted once already.

use crate::error::ApiError;
use axum::http::{HeaderMap, StatusCode};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

/// The most connections one browser session holds at once: a few per tab
/// (its stream, and on an artifact its live socket and room sockets) for
/// tens of tabs.
pub const MAX_PER_SESSION: usize = 32;
/// The most connections one address holds at once, sessions and visitors
/// with none together: room for a household or an office behind one
/// address.
pub const MAX_PER_ADDRESS: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Key {
    Address(IpAddr),
    Session(String),
}

impl Key {
    fn limit(&self) -> usize {
        match self {
            Key::Address(_) => MAX_PER_ADDRESS,
            Key::Session(_) => MAX_PER_SESSION,
        }
    }
}

/// Whom a connection is counted against: nothing on a local daemon.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SocketClient {
    keys: Vec<Key>,
}

impl SocketClient {
    /// The client of a request to `s`: on a hosted daemon its address and,
    /// with a live session, the session (its cookie's SHA-256, as the gate
    /// keeps it); on a local daemon none.
    pub fn of_request(
        s: &crate::state::AppState,
        headers: &HeaderMap,
        extensions: &axum::http::Extensions,
    ) -> SocketClient {
        let Some(h) = &s.hosted else {
            return SocketClient::default();
        };
        let address = crate::hosted::client_key(h.client(headers, extensions));
        let session = extensions
            .get::<crate::hosted::HostedRequest>()
            .and_then(|r| r.cookie_sha256.clone());
        SocketClient::of(Some(address), session)
    }

    /// The client a reopened socket's credentials name.
    pub fn of_credentials(c: &crate::identity::SocketCredentials) -> SocketClient {
        SocketClient::of(
            c.address.as_deref().and_then(|a| a.parse().ok()),
            c.session.clone(),
        )
    }

    fn of(address: Option<IpAddr>, session: Option<String>) -> SocketClient {
        SocketClient {
            keys: address
                .map(Key::Address)
                .into_iter()
                .chain(session.map(Key::Session))
                .collect(),
        }
    }

    /// The address counted, as [`crate::identity::SocketCredentials`] keeps
    /// it.
    pub fn address(&self) -> Option<String> {
        self.keys.iter().find_map(|k| match k {
            Key::Address(a) => Some(a.to_string()),
            Key::Session(_) => None,
        })
    }
}

/// Every client's connections held now.
#[derive(Default)]
pub struct SocketCaps {
    held: Mutex<HashMap<Key, usize>>,
}

/// One admitted connection: counted until it drops.
pub struct SocketSlot {
    caps: Arc<SocketCaps>,
    keys: Vec<Key>,
}

impl std::fmt::Debug for SocketSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SocketSlot")
            .field("keys", &self.keys.len())
            .finish()
    }
}

impl Drop for SocketSlot {
    fn drop(&mut self) {
        let mut held = self.caps.held.lock().unwrap_or_else(|e| e.into_inner());
        for k in &self.keys {
            if let Some(n) = held.get_mut(k) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    held.remove(k);
                }
            }
        }
    }
}

impl SocketCaps {
    /// Counts a connection of `client`, unless it holds its cap already
    /// (per address or per session).
    ///
    /// # Errors
    /// 429 `too_many_streams`, naming the cap reached.
    pub fn admit(self: &Arc<Self>, client: &SocketClient) -> Result<SocketSlot, ApiError> {
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(k) = client
            .keys
            .iter()
            .find(|k| held.get(*k).copied().unwrap_or(0) >= k.limit())
        {
            let whose = match k {
                Key::Address(_) => "this address",
                Key::Session(_) => "this browser",
            };
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "too_many_streams",
                format!(
                    "{whose} holds {} live connections to Clax already, the most it may: close a tab, or try again once one closes",
                    k.limit()
                ),
            ));
        }
        Ok(self.count(&mut held, client))
    }

    /// Counts a connection of `client` whatever it holds: a socket a host
    /// reopens, admitted once already.
    pub fn hold(self: &Arc<Self>, client: &SocketClient) -> SocketSlot {
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        self.count(&mut held, client)
    }

    fn count(
        self: &Arc<Self>,
        held: &mut HashMap<Key, usize>,
        client: &SocketClient,
    ) -> SocketSlot {
        for k in &client.keys {
            *held.entry(k.clone()).or_default() += 1;
        }
        SocketSlot {
            caps: self.clone(),
            keys: client.keys.clone(),
        }
    }

    /// How many connections `client` holds, by its most-held key.
    pub fn held(&self, client: &SocketClient) -> usize {
        let held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        client
            .keys
            .iter()
            .map(|k| held.get(k).copied().unwrap_or(0))
            .max()
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_and_an_address_are_capped_apart_and_a_dropped_slot_frees_one() {
        let caps = Arc::new(SocketCaps::default());
        let ip: IpAddr = "203.0.113.7".parse().unwrap();
        let tab = SocketClient::of(Some(ip), Some("s1".into()));
        let mut slots: Vec<SocketSlot> = (0..MAX_PER_SESSION)
            .map(|_| caps.admit(&tab).unwrap())
            .collect();
        let e = caps.admit(&tab).unwrap_err();
        assert_eq!((e.status.as_u16(), e.code), (429, "too_many_streams"));
        assert!(e.message.contains("this browser holds 32"), "{}", e.message);
        // Another session on the address is admitted, up to the address's cap.
        let other = SocketClient::of(Some(ip), Some("s2".into()));
        slots.push(caps.admit(&other).unwrap());
        slots.pop();
        slots.pop();
        assert!(caps.admit(&tab).is_ok(), "a closed one frees its place");
        let visitor = SocketClient::of(Some(ip), None);
        let mut more = Vec::new();
        while let Ok(s) = caps.admit(&visitor) {
            more.push(s);
        }
        assert_eq!(caps.held(&visitor), MAX_PER_ADDRESS);
        let e = caps.admit(&visitor).unwrap_err();
        assert!(
            e.message.contains("this address holds 128"),
            "{}",
            e.message
        );
        // A reopened socket is counted, never refused.
        let again = caps.hold(&visitor);
        assert_eq!(caps.held(&visitor), MAX_PER_ADDRESS + 1);
        drop((again, more, slots));
        assert_eq!(caps.held(&tab), 0);
        assert!(caps.held.lock().unwrap().is_empty(), "nothing left behind");
        // A local daemon's client is counted against nothing.
        let local = SocketClient::default();
        let _all: Vec<SocketSlot> = (0..MAX_PER_ADDRESS * 2)
            .map(|_| caps.admit(&local).unwrap())
            .collect();
    }
}
