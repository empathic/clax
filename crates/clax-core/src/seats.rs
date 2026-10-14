//! Live seats (spec 2026-10-11-clax-dev-hosted-design §16.1): who may hold a
//! live connection on an artifact, and who is a spectator there.
//!
//! Each artifact has a pool of people seats ([`SeatLimits::people`], 100 by
//! default) and one of agent seats ([`SeatLimits::agents`], 20). A seat is
//! keyed by who holds it ([`Claim::key`]): a person, all of whose
//! connections (tabs, browsers) hold one seat, or an agent. The owner and
//! the people the artifact names (its creator, and a person whose rights
//! name it by ID) hold reserved seats: they always get one and never count
//! against the pool. Everyone else gets a seat while one is free, first come
//! first served; guests (viewers who gave a name but are not connected
//! people) share at most [`SeatLimits::guests`] of the pool, so guests made
//! in bulk cannot lock people out. A viewer with no name takes no seat.
//!
//! A people seat is released when its last connection leaves, or once none
//! of its connections has had input (a presence report while the page is
//! visible and used, or a write) for [`SeatLimits::idle_ms`]. A person who
//! holds no seat while none is free is a spectator: read-only whatever their
//! rights, with no live connection. Agents are never spectators: their
//! writes are never refused, and their seats bound only their own live
//! connections.
//!
//! [`Seats`] is plain data with the time passed in (milliseconds): no clock,
//! no lock, no threads. The daemon wraps it in a lock with its clock.

use std::collections::{BTreeSet, HashMap};

/// People seats per artifact unless configured otherwise.
pub const DEFAULT_PEOPLE_SEATS: u32 = 100;
/// The most people seats guests hold at once unless configured otherwise.
pub const DEFAULT_GUEST_SEATS: u32 = 20;
/// Agent seats per artifact unless configured otherwise.
pub const DEFAULT_AGENT_SEATS: u32 = 20;
/// A people seat without input for this long is released: 3 minutes.
pub const SEAT_IDLE_MS: i64 = 180_000;
/// A spectator counts as viewing for this long after it was last seen (a
/// snapshot request, or a refused connection).
pub const SPECTATOR_SEEN_MS: i64 = 15_000;
/// The most spectators one artifact remembers for its viewing count.
pub const MAX_SPECTATORS: usize = 10_000;
/// The most anonymous spectators (by the tab ID their page picks) one
/// artifact counts.
pub const MAX_TAB_SPECTATORS: usize = 1000;
/// The most tab IDs one client (by address) counts for.
pub const MAX_TABS_PER_CLIENT: usize = 4;

/// What a write by a spectator is refused with.
pub const SPECTATOR_MESSAGE: &str =
    "Lots of people are here, so you're viewing; join live to comment";
/// The error code of a write refused to a spectator.
pub const SPECTATOR_CODE: &str = "spectator";
/// The error code of a live connection refused a seat.
pub const NO_SEAT_CODE: &str = "no_live_seat";
/// What a live connection refused a seat is told.
pub const NO_SEAT_MESSAGE: &str =
    "Lots of people are here, so you're viewing: comments and live activity are paused.";
/// The code a connection whose seat went without input is told.
pub const IDLE_CODE: &str = "seat_idle";

/// The environment variables that set [`SeatLimits`]: people seats, the
/// guests' share of them, agent seats, and the idle time in seconds.
pub const ENV_PEOPLE: &str = "CLAX_LIVE_PEOPLE_SEATS";
pub const ENV_GUESTS: &str = "CLAX_LIVE_GUEST_SEATS";
pub const ENV_AGENTS: &str = "CLAX_LIVE_AGENT_SEATS";
pub const ENV_IDLE_S: &str = "CLAX_LIVE_SEAT_IDLE_S";

/// How many seats an artifact has, and how long a people seat may go
/// without input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeatLimits {
    pub people: u32,
    /// The most of the people seats guests hold at once.
    pub guests: u32,
    pub agents: u32,
    pub idle_ms: i64,
}

impl Default for SeatLimits {
    fn default() -> SeatLimits {
        SeatLimits {
            people: DEFAULT_PEOPLE_SEATS,
            guests: DEFAULT_GUEST_SEATS,
            agents: DEFAULT_AGENT_SEATS,
            idle_ms: SEAT_IDLE_MS,
        }
    }
}

impl SeatLimits {
    /// The limits `get` (an environment lookup) sets through [`ENV_PEOPLE`],
    /// [`ENV_GUESTS`], [`ENV_AGENTS`] and [`ENV_IDLE_S`], each falling back
    /// to its default when unset or not a whole number (the idle time also
    /// when zero).
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> SeatLimits {
        let d = SeatLimits::default();
        let count = |k: &str, fallback: u32| {
            get(k)
                .and_then(|v| v.trim().parse::<u32>().ok())
                .unwrap_or(fallback)
        };
        let idle_s = get(ENV_IDLE_S)
            .and_then(|v| v.trim().parse::<i64>().ok())
            .filter(|s| *s > 0);
        SeatLimits {
            people: count(ENV_PEOPLE, d.people),
            guests: count(ENV_GUESTS, d.guests),
            agents: count(ENV_AGENTS, d.agents),
            idle_ms: idle_s.map_or(d.idle_ms, |s| s.saturating_mul(1000)),
        }
    }
}

/// Which pool a seat comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pool {
    People,
    Agents,
}

/// Who asks for a seat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claim {
    /// `owner`, `person:<public ID>`, `guest:<public ID>` or `agent:<ID>`.
    pub key: String,
    pub pool: Pool,
    /// The owner, or a person the artifact names: never counted against the pool.
    pub reserved: bool,
    /// A guest: the seat counts against the guests' share too.
    pub guest: bool,
}

impl Claim {
    /// The owner's claim: reserved everywhere.
    pub fn owner() -> Claim {
        Claim {
            key: OWNER_KEY.into(),
            pool: Pool::People,
            reserved: true,
            guest: false,
        }
    }

    /// A connected person's claim, reserved when the artifact names them.
    pub fn person(public_id: &str, reserved: bool) -> Claim {
        Claim {
            key: person_key(public_id),
            pool: Pool::People,
            reserved,
            guest: false,
        }
    }

    /// A guest's claim (a named viewer who is not a connected person).
    pub fn guest(public_id: &str) -> Claim {
        Claim {
            key: guest_key(public_id),
            pool: Pool::People,
            reserved: false,
            guest: true,
        }
    }

    /// An agent's claim, by its session (or another ID of its connection).
    pub fn agent(id: &str) -> Claim {
        Claim {
            key: format!("agent:{id}"),
            pool: Pool::Agents,
            reserved: false,
            guest: false,
        }
    }
}

/// The owner's seat key.
pub const OWNER_KEY: &str = "owner";

/// A connected person's seat key.
pub fn person_key(public_id: &str) -> String {
    format!("person:{public_id}")
}

/// A guest's seat key.
pub fn guest_key(public_id: &str) -> String {
    format!("guest:{public_id}")
}

/// A seat held now.
#[derive(Clone, Debug)]
struct Held {
    pool: Pool,
    reserved: bool,
    guest: bool,
    /// The latest input on the seat, in ms.
    input_ms: i64,
    /// The connections holding it.
    conns: BTreeSet<String>,
}

/// One artifact's seats and spectators.
#[derive(Default, Debug)]
struct Book {
    held: HashMap<String, Held>,
    /// Spectators by key, with when each was last seen, in ms.
    spectators: HashMap<String, i64>,
}

impl Book {
    fn free(&self, pool: Pool, guest: bool, limits: &SeatLimits) -> u32 {
        let (mut taken, mut guests) = (0u32, 0u32);
        for h in self.held.values() {
            if h.pool != pool || h.reserved {
                continue;
            }
            taken += 1;
            if h.guest {
                guests += 1;
            }
        }
        let size = match pool {
            Pool::People => limits.people,
            Pool::Agents => limits.agents,
        };
        let free = size.saturating_sub(taken);
        if guest && pool == Pool::People {
            free.min(limits.guests.saturating_sub(guests))
        } else {
            free
        }
    }

    fn may_hold(&self, c: &Claim, limits: &SeatLimits) -> bool {
        self.held.contains_key(&c.key) || c.reserved || self.free(c.pool, c.guest, limits) > 0
    }
}

/// A seat released for want of input: its artifact, key and the
/// connections that held it (to be told, and closed or unsubscribed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Released {
    pub artifact: String,
    pub key: String,
    pub conns: Vec<String>,
}

/// A held seat as a host keeps it across a restart of the process that
/// holds the seats (a Durable Object woken from hibernation):
/// [`Seats::kept`] and [`Seats::restore`]. Its connections are not kept.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KeptSeat {
    #[serde(rename = "a")]
    pub artifact: String,
    #[serde(rename = "k")]
    pub key: String,
    /// `true` for an agents' seat.
    #[serde(rename = "g", default, skip_serializing_if = "std::ops::Not::not")]
    pub agent: bool,
    #[serde(rename = "r", default, skip_serializing_if = "std::ops::Not::not")]
    pub reserved: bool,
    #[serde(rename = "q", default, skip_serializing_if = "std::ops::Not::not")]
    pub guest: bool,
    /// The latest input, in ms.
    #[serde(rename = "t")]
    pub input_ms: i64,
}

/// The connection key a restored seat is held under until its holder's
/// own connection takes it ([`Seats::restore`]).
pub const RESTORED_CONN: &str = "restored:";

/// Every artifact's seats (see the module docs).
#[derive(Debug, Default)]
pub struct Seats {
    limits: SeatLimits,
    books: HashMap<String, Book>,
    /// The claims of connections that dropped and may come back (a stream
    /// that may be resumed), by connection: their seats are released, and
    /// [`Seats::unpark`] takes them again.
    parked: HashMap<String, Vec<(String, Claim)>>,
}

impl Seats {
    pub fn new(limits: SeatLimits) -> Seats {
        Seats {
            limits,
            ..Seats::default()
        }
    }

    pub fn limits(&self) -> SeatLimits {
        self.limits
    }

    /// Changes the limits; seats held stay held.
    pub fn set_limits(&mut self, limits: SeatLimits) {
        self.limits = limits;
    }

    /// Releases every people seat without input for the idle time at
    /// `now_ms`, and forgets the spectators not seen for
    /// [`SPECTATOR_SEEN_MS`] (an artifact no one polls any more keeps none).
    pub fn release_idle(&mut self, now_ms: i64) -> Vec<Released> {
        let idle = self.limits.idle_ms;
        let mut out = Vec::new();
        for (aid, book) in &mut self.books {
            book.spectators
                .retain(|_, t| now_ms - *t <= SPECTATOR_SEEN_MS);
            let gone: Vec<String> = book
                .held
                .iter()
                .filter(|(_, h)| h.pool == Pool::People && now_ms - h.input_ms >= idle)
                .map(|(k, _)| k.clone())
                .collect();
            for key in gone {
                if let Some(h) = book.held.remove(&key) {
                    out.push(Released {
                        artifact: aid.clone(),
                        key,
                        conns: h.conns.into_iter().collect(),
                    });
                }
            }
        }
        self.books
            .retain(|_, b| !b.held.is_empty() || !b.spectators.is_empty());
        out
    }

    /// When the next people seat goes idle, in ms; `None` with none held.
    pub fn next_idle_at(&self) -> Option<i64> {
        self.books
            .values()
            .flat_map(|b| b.held.values())
            .filter(|h| h.pool == Pool::People)
            .map(|h| h.input_ms + self.limits.idle_ms)
            .min()
    }

    /// The free seats of `pool` on `aid`; for a guest, no more than the
    /// guests' share leaves.
    pub fn free(&self, aid: &str, pool: Pool, guest: bool) -> u32 {
        match self.books.get(aid) {
            Some(b) => b.free(pool, guest, &self.limits),
            None => Book::default().free(pool, guest, &self.limits),
        }
    }

    /// Whether `c` holds or may get a seat on `aid`: one it holds already
    /// (another connection of the same person), a reserved one, or a free one.
    pub fn may_hold(&self, aid: &str, c: &Claim) -> bool {
        match self.books.get(aid) {
            Some(b) => b.may_hold(c, &self.limits),
            None => Book::default().may_hold(c, &self.limits),
        }
    }

    /// Whether the seat `key` is held on `aid` now.
    pub fn seated(&self, aid: &str, key: &str) -> bool {
        self.books
            .get(aid)
            .is_some_and(|b| b.held.contains_key(key))
    }

    /// Connection `conn` takes `c`'s seat on `aid` at `now_ms`, when
    /// [`Seats::may_hold`] allows: answers whether it holds the seat now. A
    /// new seat starts with input at `now_ms`; the claim's spectator entry
    /// goes.
    pub fn take(&mut self, aid: &str, c: &Claim, conn: &str, now_ms: i64) -> bool {
        if !self.may_hold(aid, c) {
            return false;
        }
        let book = self.books.entry(aid.to_string()).or_default();
        book.spectators.remove(&c.key);
        let h = book.held.entry(c.key.clone()).or_insert_with(|| Held {
            pool: c.pool,
            reserved: c.reserved,
            guest: c.guest,
            input_ms: now_ms,
            conns: BTreeSet::new(),
        });
        h.reserved |= c.reserved;
        // A restored seat is its holder's again once they connect.
        h.conns.retain(|k| !k.starts_with(RESTORED_CONN));
        h.conns.insert(conn.to_string());
        true
    }

    /// Every seat held now, for a host to keep ([`KeptSeat`]), in a stable
    /// order (by artifact, then key).
    pub fn kept(&self) -> Vec<KeptSeat> {
        let mut out: Vec<KeptSeat> = self
            .books
            .iter()
            .flat_map(|(aid, b)| {
                b.held.iter().map(move |(key, h)| KeptSeat {
                    artifact: aid.clone(),
                    key: key.clone(),
                    agent: h.pool == Pool::Agents,
                    reserved: h.reserved,
                    guest: h.guest,
                    input_ms: h.input_ms,
                })
            })
            .collect();
        out.sort_by(|a, b| (&a.artifact, &a.key).cmp(&(&b.artifact, &b.key)));
        out
    }

    /// Holds `kept` again, each under [`RESTORED_CONN`] and its key, with
    /// its latest input, before anyone else can take a seat: a restarted
    /// host's seated people stay seated while their connections come back
    /// (the first to take the seat replaces the placeholder), and a seat
    /// whose holder does not come back is released when it goes idle, as
    /// any is. A seat already held is left as it is. Agents' seats are not
    /// held again: none goes idle, so a placeholder whose agent never came
    /// back would hold its seat for good; an agent's connection takes one
    /// anew.
    pub fn restore(&mut self, kept: &[KeptSeat]) {
        for k in kept.iter().filter(|k| !k.agent) {
            let book = self.books.entry(k.artifact.clone()).or_default();
            book.held.entry(k.key.clone()).or_insert_with(|| Held {
                pool: if k.agent { Pool::Agents } else { Pool::People },
                reserved: k.reserved,
                guest: k.guest,
                input_ms: k.input_ms,
                conns: BTreeSet::from([format!("{RESTORED_CONN}{}", k.key)]),
            });
        }
    }

    /// Connection `conn` leaves its seat on `aid`; the seat is released
    /// with its last connection. Answers whether the connection held one.
    pub fn leave(&mut self, aid: &str, conn: &str) -> bool {
        let Some(book) = self.books.get_mut(aid) else {
            return false;
        };
        let mut was = false;
        book.held.retain(|_, h| {
            was |= h.conns.remove(conn);
            !h.conns.is_empty()
        });
        if book.held.is_empty() && book.spectators.is_empty() {
            self.books.remove(aid);
        }
        if let Some(p) = self.parked.get_mut(conn) {
            p.retain(|(a, _)| a != aid);
        }
        was
    }

    /// The claims connection `conn` holds seats with, by artifact.
    fn claims_of(&self, conn: &str) -> Vec<(String, Claim)> {
        let mut out = Vec::new();
        for (aid, book) in &self.books {
            for (key, h) in &book.held {
                if h.conns.contains(conn) {
                    out.push((
                        aid.clone(),
                        Claim {
                            key: key.clone(),
                            pool: h.pool,
                            reserved: h.reserved,
                            guest: h.guest,
                        },
                    ));
                }
            }
        }
        out
    }

    /// Connection `conn` dropped but may come back: its seats are released
    /// and its claims kept for [`Seats::unpark`].
    pub fn park(&mut self, conn: &str) {
        let claims = self.claims_of(conn);
        if claims.is_empty() {
            return;
        }
        for (aid, _) in &claims {
            self.leave(aid, conn);
        }
        self.parked.insert(conn.to_string(), claims);
    }

    /// Connection `conn` came back: it takes its parked seats again where
    /// it may. Answers the artifacts it got no seat on.
    pub fn unpark(&mut self, conn: &str, now_ms: i64) -> Vec<String> {
        let Some(claims) = self.parked.remove(conn) else {
            return Vec::new();
        };
        claims
            .into_iter()
            .filter(|(aid, c)| !self.take(aid, c, conn, now_ms))
            .map(|(aid, _)| aid)
            .collect()
    }

    /// Connection `conn` is gone for good: its seats and parked claims go.
    pub fn forget(&mut self, conn: &str) {
        self.parked.remove(conn);
        for (aid, _) in self.claims_of(conn) {
            self.leave(&aid, conn);
        }
    }

    /// Input on seat `key` of `aid` at `now_ms`. Answers whether the seat is held.
    pub fn touch(&mut self, aid: &str, key: &str, now_ms: i64) -> bool {
        match self.books.get_mut(aid).and_then(|b| b.held.get_mut(key)) {
            Some(h) => {
                h.input_ms = h.input_ms.max(now_ms);
                true
            }
            None => false,
        }
    }

    /// Notes spectator `key` of `aid` as viewing at `now_ms` (the viewing
    /// count includes spectators). An anonymous spectator's key
    /// (`tab:<client>:<tab>`) is one its page picks, so each client holds at
    /// most [`MAX_TABS_PER_CLIENT`] and all of them at most
    /// [`MAX_TAB_SPECTATORS`]; past either, or past [`MAX_SPECTATORS`], it
    /// is not counted. A seated key is not a spectator.
    pub fn see_spectator(&mut self, aid: &str, key: &str, now_ms: i64) {
        let book = self.books.entry(aid.to_string()).or_default();
        if book.held.contains_key(key) {
            return;
        }
        if let Some(t) = book.spectators.get_mut(key) {
            *t = now_ms;
            return;
        }
        book.spectators
            .retain(|_, t| now_ms - *t <= SPECTATOR_SEEN_MS);
        if book.spectators.len() >= MAX_SPECTATORS {
            return;
        }
        if let Some(rest) = key.strip_prefix("tab:") {
            let client = rest.rsplit_once(':').map_or("", |(c, _)| c);
            let mine = format!("tab:{client}:");
            let tabs = book.spectators.keys().filter(|k| k.starts_with("tab:"));
            let (all, of_client) = tabs.fold((0, 0), |(a, m), k| {
                (a + 1, m + usize::from(k.starts_with(&mine)))
            });
            if all >= MAX_TAB_SPECTATORS || of_client >= MAX_TABS_PER_CLIENT {
                return;
            }
        }
        book.spectators.insert(key.to_string(), now_ms);
    }

    /// Everyone viewing `aid` at `now_ms`: the people seats held, and each
    /// spectator seen in the last [`SPECTATOR_SEEN_MS`].
    pub fn viewing(&mut self, aid: &str, now_ms: i64) -> u32 {
        let Some(book) = self.books.get_mut(aid) else {
            return 0;
        };
        book.spectators
            .retain(|_, t| now_ms - *t <= SPECTATOR_SEEN_MS);
        let people = book
            .held
            .values()
            .filter(|h| h.pool == Pool::People)
            .count();
        let spectators = book
            .spectators
            .keys()
            .filter(|k| !book.held.contains_key(*k))
            .count();
        u32::try_from(people + spectators).unwrap_or(u32::MAX)
    }

    /// How many seats of `pool` are held on `aid` (reserved ones included).
    pub fn held(&self, aid: &str, pool: Pool) -> u32 {
        self.books.get(aid).map_or(0, |b| {
            u32::try_from(b.held.values().filter(|h| h.pool == pool).count()).unwrap_or(u32::MAX)
        })
    }
}

#[cfg(test)]
mod restore_tests {
    use super::*;

    #[test]
    fn restored_seats_stay_seated_ahead_of_newcomers_until_their_holders_return_or_idle() {
        let limits = SeatLimits {
            people: 1,
            ..SeatLimits::default()
        };
        let mut s = Seats::new(limits);
        assert!(s.take("a", &Claim::person("p1", false), "stream:1", 1_000));
        s.touch("a", &person_key("p1"), 2_000);
        let kept = s.kept();
        assert_eq!(kept[0].input_ms, 2_000);

        let mut woken = Seats::new(limits);
        woken.restore(&kept);
        // A newcomer finds the one seat held; its holder takes it back.
        assert!(!woken.take("a", &Claim::person("p2", false), "stream:9", 3_000));
        assert!(woken.take("a", &Claim::person("p1", false), "stream:7", 3_000));
        assert_eq!(woken.kept(), kept, "the same seat, its input kept");
        // Leaving with its own connection releases it: no placeholder left.
        woken.leave("a", "stream:7");
        assert!(!woken.seated("a", &person_key("p1")));

        // A holder who never returns is released when the seat goes idle.
        let mut idle = Seats::new(limits);
        idle.restore(&kept);
        let gone = idle.release_idle(2_000 + limits.idle_ms);
        assert_eq!(gone.len(), 1);
        assert_eq!(gone[0].conns, vec![format!("{RESTORED_CONN}person:p1")]);

        // An agent's seat, which never goes idle, is not held again.
        let mut agents = Seats::new(limits);
        assert!(agents.take("a", &Claim::agent("s1"), "s1", 1_000));
        let mut woken = Seats::new(limits);
        woken.restore(&agents.kept());
        assert!(woken.kept().is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "7q3k9mzx2b4t";

    fn seats(people: u32, guests: u32) -> Seats {
        Seats::new(SeatLimits {
            people,
            guests,
            agents: 2,
            idle_ms: 1000,
        })
    }

    #[test]
    fn people_take_free_seats_first_come_first_served() {
        let mut s = seats(2, 2);
        assert!(s.take(A, &Claim::person("p1", false), "c1", 0));
        assert!(s.take(A, &Claim::person("p2", false), "c2", 0));
        assert!(!s.take(A, &Claim::person("p3", false), "c3", 0));
        assert_eq!(s.free(A, Pool::People, false), 0);
        assert!(!s.seated(A, &person_key("p3")));
        // A seat freed goes to the next who asks.
        assert!(s.leave(A, "c1"));
        assert!(s.take(A, &Claim::person("p3", false), "c3", 0));
    }

    #[test]
    fn reserved_seats_never_count_against_the_pool() {
        let mut s = seats(1, 1);
        assert!(s.take(A, &Claim::owner(), "o", 0));
        assert!(s.take(A, &Claim::person("named", true), "n", 0));
        assert_eq!(s.free(A, Pool::People, false), 1, "two reserved, one free");
        assert!(s.take(A, &Claim::person("p1", false), "c1", 0));
        assert!(!s.take(A, &Claim::person("p2", false), "c2", 0));
        // The full pool still seats the owner and a named person.
        assert!(s.take(A, &Claim::owner(), "o2", 0));
        assert!(s.take(A, &Claim::person("named2", true), "n2", 0));
        assert!(s.may_hold(A, &Claim::person("named3", true)));
    }

    #[test]
    fn one_person_holds_one_seat_across_connections() {
        let mut s = seats(1, 1);
        assert!(s.take(A, &Claim::person("p1", false), "tab1", 0));
        assert!(
            s.take(A, &Claim::person("p1", false), "tab2", 0),
            "a second tab"
        );
        assert_eq!(s.held(A, Pool::People), 1);
        assert!(!s.take(A, &Claim::person("p2", false), "x", 0));
        assert!(s.leave(A, "tab1"));
        assert!(
            s.seated(A, &person_key("p1")),
            "held while a connection stays"
        );
        assert!(s.leave(A, "tab2"));
        assert!(!s.seated(A, &person_key("p1")));
        assert!(s.take(A, &Claim::person("p2", false), "x", 0));
    }

    #[test]
    fn guests_share_at_most_their_part_of_the_pool() {
        let mut s = seats(3, 1);
        assert!(s.take(A, &Claim::guest("g1"), "g1", 0));
        assert!(
            !s.take(A, &Claim::guest("g2"), "g2", 0),
            "the guests' share is full"
        );
        assert_eq!(s.free(A, Pool::People, true), 0);
        assert_eq!(s.free(A, Pool::People, false), 2);
        assert!(s.take(A, &Claim::person("p1", false), "p1", 0));
        assert!(s.take(A, &Claim::person("p2", false), "p2", 0));
    }

    #[test]
    fn agents_have_a_pool_of_their_own() {
        let mut s = seats(1, 1);
        assert!(s.take(A, &Claim::person("p1", false), "p1", 0));
        assert!(s.take(A, &Claim::agent("s1"), "s1", 0));
        assert!(s.take(A, &Claim::agent("s2"), "s2", 0));
        assert!(!s.take(A, &Claim::agent("s3"), "s3", 0));
        assert_eq!(s.free(A, Pool::People, false), 0);
    }

    #[test]
    fn idle_people_seats_are_released_and_input_keeps_them() {
        let mut s = seats(2, 2);
        s.take(A, &Claim::person("p1", false), "c1", 0);
        s.take(A, &Claim::person("p2", false), "c2", 0);
        s.take(A, &Claim::agent("s1"), "s1", 0);
        assert!(s.touch(A, &person_key("p2"), 600));
        assert_eq!(s.next_idle_at(), Some(1000));
        assert!(s.release_idle(999).is_empty());
        let gone = s.release_idle(1000);
        assert_eq!(
            gone,
            vec![Released {
                artifact: A.into(),
                key: person_key("p1"),
                conns: vec!["c1".into()]
            }]
        );
        assert!(s.seated(A, &person_key("p2")));
        assert!(s.seated(A, "agent:s1"), "agent seats never go idle");
        assert_eq!(s.release_idle(1600).len(), 1);
        assert!(
            !s.touch(A, &person_key("p2"), 1700),
            "a released seat takes no input"
        );
    }

    #[test]
    fn a_parked_connection_takes_its_seat_again_while_one_is_free() {
        let mut s = seats(1, 1);
        s.take(A, &Claim::person("p1", false), "stream", 0);
        s.park("stream");
        assert!(!s.seated(A, &person_key("p1")));
        assert!(s.unpark("stream", 5).is_empty());
        assert!(s.seated(A, &person_key("p1")));
        s.park("stream");
        assert!(s.take(A, &Claim::person("p2", false), "other", 5));
        assert_eq!(s.unpark("stream", 6), vec![A.to_string()]);
        s.forget("other");
        assert!(!s.seated(A, &person_key("p2")));
    }

    #[test]
    fn viewing_counts_seats_and_recent_spectators() {
        let mut s = seats(1, 1);
        s.take(A, &Claim::person("p1", false), "c1", 0);
        s.see_spectator(A, &person_key("p2"), 0);
        s.see_spectator(A, &person_key("p1"), 0);
        assert_eq!(
            s.viewing(A, 0),
            2,
            "a seated person is not also a spectator"
        );
        assert_eq!(s.viewing(A, SPECTATOR_SEEN_MS + 1), 1);
        for t in 0..10 {
            s.see_spectator(A, &format!("tab:1.2.3.4:{t}"), 0);
        }
        assert_eq!(s.viewing(A, 0), 1 + MAX_TABS_PER_CLIENT as u32);
    }

    #[test]
    fn spectators_no_one_sees_any_more_are_forgotten_with_their_artifact() {
        let mut s = seats(1, 1);
        s.see_spectator(A, &person_key("p2"), 0);
        assert_eq!(s.books.len(), 1);
        assert!(s.release_idle(SPECTATOR_SEEN_MS).is_empty());
        assert_eq!(s.books.len(), 1, "seen within the window");
        s.release_idle(SPECTATOR_SEEN_MS + 1);
        assert!(s.books.is_empty());
    }

    #[test]
    fn limits_come_from_the_environment() {
        let env = |k: &str| match k {
            ENV_PEOPLE => Some("1".into()),
            ENV_GUESTS => Some("x".into()),
            ENV_IDLE_S => Some("6".into()),
            _ => None,
        };
        let l = SeatLimits::from_env(env);
        assert_eq!(
            l,
            SeatLimits {
                people: 1,
                guests: DEFAULT_GUEST_SEATS,
                agents: DEFAULT_AGENT_SEATS,
                idle_ms: 6000
            }
        );
        assert_eq!(SeatLimits::from_env(|_| None), SeatLimits::default());
    }
}
