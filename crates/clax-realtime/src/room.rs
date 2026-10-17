//! The in-memory hub of the `room` capability (spec §9). Nothing here is
//! persisted and nothing is carried on `/api/events`.
//!
//! Rooms are keyed by artifact, not by version: pages of every version of
//! one artifact share its lobby and its named rooms, so a viewer still on an
//! older version meets the viewers on the newest one. Each artifact has a
//! lobby (`None`) and any number of named rooms (`Some(name)`); a room exists
//! while it has members.
//!
//! A member is one socket in one room ([`Membership`]); dropping it removes
//! the member and tells the room it left. Peer labels are unique per
//! artifact: a socket that takes a label already held ([`Rooms::claim`])
//! wakes the older socket so it closes `replaced`, and a member entering a
//! room under a label already present takes that entry's place without a
//! `left` (the room sees one upsert).

use clax_core::room::MAX_SNAPSHOT_PEERS;
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, broadcast};

/// Frames a room buffers for a slow socket before it falls behind (and is
/// sent a fresh `peers` snapshot).
const ROOM_BUFFER: usize = 256;

/// Who a socket is: its peer label, its sender's `by` (the viewer public ID
/// when the artifact declares `user`, else `None`) and its viewer, which
/// decides `isMe` across one viewer's tabs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Who {
    pub peer: String,
    pub by: Option<String>,
    pub viewer: Option<String>,
}

/// The sender fields of a peer or message, as one receiver `me` sees them.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sender {
    pub peer: String,
    pub by: Option<String>,
    /// The same socket, or another socket of the same viewer.
    pub is_me: bool,
    /// The same socket.
    pub same_tab: bool,
    /// Always `"viewer"`: no agent peer joins a room.
    pub kind: &'static str,
    /// Always `false`.
    pub guest: bool,
}

impl Sender {
    pub fn render(who: &Who, me: &Who) -> Sender {
        let same_tab = who.peer == me.peer;
        let same_viewer = who.viewer.is_some() && who.viewer == me.viewer;
        Sender {
            peer: who.peer.clone(),
            by: who.by.clone(),
            is_me: same_tab || same_viewer,
            same_tab,
            kind: "viewer",
            guest: false,
        }
    }
}

/// What a room broadcasts to its members; each socket renders it for itself.
#[derive(Debug)]
pub enum Frame {
    /// A member entered or changed its presence (an upsert).
    Peer {
        who: Who,
        presence: Map<String, Value>,
    },
    /// A member left.
    Left { peer: String },
    /// A message on a topic; `data` as the page gave it.
    Msg {
        who: Who,
        topic: String,
        data: Option<Value>,
    },
}

type Key = (String, Option<String>);

/// A member and its presence.
pub type Entry = (Who, Map<String, Value>);

struct Room {
    tx: broadcast::Sender<Arc<Frame>>,
    /// Members in the order they entered, by entry serial.
    members: BTreeMap<u64, Entry>,
}

#[derive(Default)]
struct Inner {
    rooms: HashMap<Key, Room>,
    /// The current socket of each `(artifact, label)`.
    claims: HashMap<(String, String), Arc<Notify>>,
    serial: u64,
}

/// Every live room of the daemon.
#[derive(Default)]
pub struct Rooms {
    inner: Arc<Mutex<Inner>>,
}

/// A socket's hold on its peer label. `replaced` is notified when a newer
/// socket claims the same label.
pub struct Claim {
    inner: Arc<Mutex<Inner>>,
    key: (String, String),
    pub replaced: Arc<Notify>,
}

impl Drop for Claim {
    fn drop(&mut self) {
        let mut g = lock(&self.inner);
        if g.claims
            .get(&self.key)
            .is_some_and(|n| Arc::ptr_eq(n, &self.replaced))
        {
            g.claims.remove(&self.key);
        }
    }
}

fn lock(m: &Mutex<Inner>) -> std::sync::MutexGuard<'_, Inner> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Rooms {
    /// Claims `peer` on artifact `aid` for a new socket, waking the socket
    /// that held it.
    pub fn claim(&self, aid: &str, peer: &str) -> Claim {
        let key = (aid.to_string(), peer.to_string());
        let replaced = Arc::new(Notify::new());
        if let Some(old) = lock(&self.inner)
            .claims
            .insert(key.clone(), replaced.clone())
        {
            old.notify_one();
        }
        Claim {
            inner: self.inner.clone(),
            key,
            replaced,
        }
    }

    /// Enters `who` into artifact `aid`'s lobby (`room: None`) or named room
    /// with an empty presence, tells the room, and subscribes to it (the
    /// receiver does not see its own entry).
    pub fn enter(
        &self,
        aid: &str,
        room: Option<&str>,
        who: Who,
    ) -> (Membership, broadcast::Receiver<Arc<Frame>>) {
        let key: Key = (aid.to_string(), room.map(str::to_string));
        let mut g = lock(&self.inner);
        g.serial += 1;
        let serial = g.serial;
        let r = g.rooms.entry(key.clone()).or_insert_with(|| Room {
            tx: broadcast::channel(ROOM_BUFFER).0,
            members: BTreeMap::new(),
        });
        r.members.retain(|_, (w, _)| w.peer != who.peer);
        let _ = r.tx.send(Arc::new(Frame::Peer {
            who: who.clone(),
            presence: Map::new(),
        }));
        r.members.insert(serial, (who, Map::new()));
        let rx = r.tx.subscribe();
        drop(g);
        (
            Membership {
                inner: self.inner.clone(),
                key,
                serial,
            },
            rx,
        )
    }

    /// Closes every socket of viewer `public_id` (a viewer that no longer
    /// exists: it was folded into the owner): its entries leave their rooms
    /// at once, the rooms are told, and each socket is woken to end, so the
    /// page reconnects as the viewer it is now. Returns how many entries left.
    pub fn evict_viewer(&self, public_id: &str) -> usize {
        let mut g = lock(&self.inner);
        let mut left = Vec::new();
        for ((aid, _), r) in g.rooms.iter_mut() {
            let gone: Vec<u64> = r
                .members
                .iter()
                .filter(|(_, (w, _))| w.viewer.as_deref() == Some(public_id))
                .map(|(s, _)| *s)
                .collect();
            for s in gone {
                if let Some((who, _)) = r.members.remove(&s) {
                    let _ = r.tx.send(Arc::new(Frame::Left {
                        peer: who.peer.clone(),
                    }));
                    left.push((aid.clone(), who.peer));
                }
            }
        }
        g.rooms.retain(|_, r| !r.members.is_empty());
        for key in &left {
            if let Some(n) = g.claims.get(key) {
                n.notify_one();
            }
        }
        left.len()
    }

    /// The number of live rooms (for tests).
    pub fn len(&self) -> usize {
        lock(&self.inner).rooms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// One socket's place in one room; dropping it leaves the room.
pub struct Membership {
    inner: Arc<Mutex<Inner>>,
    key: Key,
    serial: u64,
}

impl Membership {
    /// Replaces this member's presence and tells the room.
    pub fn set_presence(&self, presence: Map<String, Value>) {
        let mut g = lock(&self.inner);
        let Some(r) = g.rooms.get_mut(&self.key) else {
            return;
        };
        let Some((who, p)) = r.members.get_mut(&self.serial) else {
            return;
        };
        p.clone_from(&presence);
        let frame = Frame::Peer {
            who: who.clone(),
            presence,
        };
        let _ = r.tx.send(Arc::new(frame));
    }

    /// Sends a message from this member to the room (itself included).
    pub fn emit(&self, topic: String, data: Option<Value>) {
        let g = lock(&self.inner);
        let Some(r) = g.rooms.get(&self.key) else {
            return;
        };
        let Some((who, _)) = r.members.get(&self.serial) else {
            return;
        };
        let _ = r.tx.send(Arc::new(Frame::Msg {
            who: who.clone(),
            topic,
            data,
        }));
    }

    /// The room's members in the order they entered, at most
    /// [`MAX_SNAPSHOT_PEERS`] of them, this member always among them.
    pub fn snapshot(&self) -> Vec<Entry> {
        let g = lock(&self.inner);
        let Some(r) = g.rooms.get(&self.key) else {
            return Vec::new();
        };
        let others = r
            .members
            .iter()
            .filter(|(s, _)| **s != self.serial)
            .take(MAX_SNAPSHOT_PEERS - 1);
        let mut picked: Vec<(&u64, &Entry)> = others.collect();
        if let Some(me) = r.members.get_key_value(&self.serial) {
            picked.push(me);
        }
        picked.sort_by_key(|(s, _)| **s);
        picked.into_iter().map(|(_, m)| m.clone()).collect()
    }
}

impl Drop for Membership {
    fn drop(&mut self) {
        let mut g = lock(&self.inner);
        let Some(r) = g.rooms.get_mut(&self.key) else {
            return;
        };
        // Absent when a newer socket under the same label took this entry.
        if let Some((who, _)) = r.members.remove(&self.serial) {
            let _ = r.tx.send(Arc::new(Frame::Left { peer: who.peer }));
        }
        if r.members.is_empty() {
            g.rooms.remove(&self.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn who(peer: &str, viewer: Option<&str>) -> Who {
        Who {
            peer: peer.into(),
            by: None,
            viewer: viewer.map(Into::into),
        }
    }

    #[tokio::test]
    async fn evicting_a_viewer_closes_its_sockets_and_tells_the_room() {
        let rooms = Rooms::default();
        let claim = rooms.claim("a1", "aaaaaaaaaaaaaaaa");
        let (_gone, _) = rooms.enter("a1", None, who("aaaaaaaaaaaaaaaa", Some("u_old")));
        let (stay, mut rx) = rooms.enter("a1", None, who("bbbbbbbbbbbbbbbb", Some("u_other")));
        assert_eq!(rooms.evict_viewer("u_old"), 1);
        match &*rx.recv().await.unwrap() {
            Frame::Left { peer } => assert_eq!(peer, "aaaaaaaaaaaaaaaa"),
            f => panic!("{f:?}"),
        }
        assert_eq!(stay.snapshot().len(), 1);
        // The evicted socket is woken to end.
        tokio::time::timeout(std::time::Duration::from_secs(1), claim.replaced.notified())
            .await
            .unwrap();
        assert_eq!(rooms.evict_viewer("u_old"), 0);
    }

    #[test]
    fn is_me_spans_a_viewers_sockets_and_same_tab_is_one_socket() {
        let a = who("a", Some("v1"));
        let s = Sender::render(&who("b", Some("v1")), &a);
        assert!(s.is_me && !s.same_tab);
        let s = Sender::render(&a, &a);
        assert!(s.is_me && s.same_tab);
        let s = Sender::render(&who("b", None), &who("a", None));
        assert!(!s.is_me && !s.same_tab);
    }

    #[test]
    fn members_see_each_other_and_leaving_empties_the_room() {
        let rooms = Rooms::default();
        let (ma, mut ra) = rooms.enter("x", None, who("a", None));
        let (mb, _rb) = rooms.enter("x", None, who("b", None));
        assert!(matches!(&*ra.try_recv().unwrap(), Frame::Peer { who, .. } if who.peer == "b"));
        assert_eq!(mb.snapshot().len(), 2);
        drop(mb);
        assert!(matches!(&*ra.try_recv().unwrap(), Frame::Left { peer } if peer == "b"));
        drop(ma);
        assert!(rooms.is_empty());
    }

    #[test]
    fn rooms_are_per_artifact_and_name() {
        let rooms = Rooms::default();
        let (_a, _) = rooms.enter("x", None, who("a", None));
        let (_b, _) = rooms.enter("x", Some("t"), who("a", None));
        let (c, _) = rooms.enter("y", None, who("c", None));
        assert_eq!(rooms.len(), 3);
        assert_eq!(c.snapshot().len(), 1);
    }

    #[test]
    fn a_label_reentering_takes_the_old_entry_without_a_left() {
        let rooms = Rooms::default();
        let (_o, mut ro) = rooms.enter("x", None, who("o", None));
        let (old, _) = rooms.enter("x", None, who("a", None));
        let (new, _) = rooms.enter("x", None, who("a", None));
        drop(old);
        let mut frames = Vec::new();
        while let Ok(f) = ro.try_recv() {
            frames.push(f);
        }
        assert!(frames.iter().all(|f| !matches!(&**f, Frame::Left { .. })));
        assert_eq!(new.snapshot().len(), 2);
    }

    #[test]
    fn a_newer_claim_wakes_the_older_and_outlives_it() {
        let rooms = Rooms::default();
        let old = rooms.claim("x", "a");
        let new = rooms.claim("x", "a");
        assert!(
            futures::FutureExt::now_or_never(old.replaced.notified()).is_some(),
            "the old claim is woken"
        );
        drop(old);
        let g = lock(&rooms.inner);
        assert!(Arc::ptr_eq(
            &g.claims[&("x".into(), "a".into())],
            &new.replaced
        ));
    }

    #[test]
    fn snapshots_hold_at_most_256_peers_with_the_receiver() {
        let rooms = Rooms::default();
        let held: Vec<_> = (0..300)
            .map(|i| rooms.enter("x", None, who(&format!("p{i}"), None)).0)
            .collect();
        let last = held.last().unwrap();
        let snap = last.snapshot();
        assert_eq!(snap.len(), MAX_SNAPSHOT_PEERS);
        assert!(snap.iter().any(|(w, _)| w.peer == "p299"));
    }

    #[test]
    fn presence_is_broadcast_and_kept_for_snapshots() {
        let rooms = Rooms::default();
        let (a, _) = rooms.enter("x", None, who("a", None));
        let (_b, mut rb) = rooms.enter("x", None, who("b", None));
        let mut p = Map::new();
        p.insert("k".into(), Value::from(1));
        a.set_presence(p.clone());
        assert!(matches!(&*rb.try_recv().unwrap(), Frame::Peer { presence, .. } if *presence == p));
        assert_eq!(a.snapshot()[0].1, p);
    }
}
