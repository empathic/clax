//! Per-person flags (migration 50, spec 2026-10-21-claxbot-design §5): a
//! generic `(person, flag, value, set_by, set_at)` table whose flag names
//! are checked here ([`Flag`]). A person without a row has the flag's
//! default ([`Flag::default_for`]). Admins set them; the routes check that.

use super::Store;
use super::roles::{OWNER, check_live_person, set_by};
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord};
use clax_core::{CoreError, Result};
use rusqlite::{Connection, params};
use serde::Serialize;

/// A known flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Flag {
    /// The person may use ClaxBot. On for the owner, off for everyone else,
    /// until an admin sets it.
    Claxbot,
    /// ClaxBot meters the person's usage but never stops them for credits.
    ClaxbotUnenforced,
    /// The person may report problems with Clax itself, through the
    /// instance's feedback queue (spec 2026-10-29-feedback-loop-design
    /// §10). Off for everyone, the owner too, until an admin sets it.
    Feedback,
}

impl Flag {
    /// Every flag.
    pub const ALL: &'static [Flag] = &[Flag::Claxbot, Flag::ClaxbotUnenforced, Flag::Feedback];

    /// The flag's stored name.
    pub fn as_str(self) -> &'static str {
        match self {
            Flag::Claxbot => "claxbot",
            Flag::ClaxbotUnenforced => "claxbot_unenforced",
            Flag::Feedback => "feedback",
        }
    }

    /// The flag named `s`.
    ///
    /// # Errors
    /// `unknown_flag`, listing the flags.
    pub fn parse(s: &str) -> Result<Flag> {
        Flag::ALL
            .iter()
            .copied()
            .find(|f| f.as_str() == s)
            .ok_or_else(|| {
                CoreError::invalid(
                    "unknown_flag",
                    format!(
                        "no flag is named '{s}': the flags are claxbot, claxbot_unenforced and feedback"
                    ),
                )
            })
    }

    /// The value of the flag for `person` (a person key) with no row:
    /// `claxbot` is on for the owner alone; everything else, `feedback`
    /// included, is off.
    pub fn default_for(self, person: &str) -> bool {
        matches!(self, Flag::Claxbot) && person == OWNER
    }
}

/// A person's flags, their defaults filled in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Flags {
    pub claxbot: bool,
    pub claxbot_unenforced: bool,
    pub feedback: bool,
}

impl Flags {
    /// The value of `flag`.
    pub fn get(&self, flag: Flag) -> bool {
        match flag {
            Flag::Claxbot => self.claxbot,
            Flag::ClaxbotUnenforced => self.claxbot_unenforced,
            Flag::Feedback => self.feedback,
        }
    }

    fn set(&mut self, flag: Flag, v: bool) {
        match flag {
            Flag::Claxbot => self.claxbot = v,
            Flag::ClaxbotUnenforced => self.claxbot_unenforced = v,
            Flag::Feedback => self.feedback = v,
        }
    }
}

/// `person`'s flags on `c`. A stored row whose name this build does not
/// know is ignored.
pub(crate) fn flags_in(c: &Connection, person: &str) -> Result<Flags> {
    let mut flags = Flags {
        claxbot: Flag::Claxbot.default_for(person),
        claxbot_unenforced: Flag::ClaxbotUnenforced.default_for(person),
        feedback: Flag::Feedback.default_for(person),
    };
    let mut q = c.prepare_cached("SELECT flag, value FROM person_flags WHERE person = ?1")?;
    let rows = q.query_map(params![person], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?))
    })?;
    for row in rows {
        let (name, v) = row?;
        if let Ok(f) = Flag::parse(&name) {
            flags.set(f, v);
        }
    }
    Ok(flags)
}

impl Store {
    /// `person`'s flags (a person key), their defaults filled in.
    ///
    /// # Errors
    /// The store's.
    pub fn flags_of(&self, person: &str) -> Result<Flags> {
        self.with_read(|c| flags_in(c, person))
    }

    /// Sets each of `changes` on `person`, in one transaction, recording
    /// `admin.flag {person, flag, value}` under `ctx` for each that
    /// changes the person's value. Returns the person's flags after.
    ///
    /// # Errors
    /// `NotFound` for no live person; the store's.
    pub fn set_flags(
        &self,
        ctx: &AuditCtx,
        person: &str,
        changes: &[(Flag, bool)],
    ) -> Result<Flags> {
        self.with_tx(|tx| {
            check_live_person(tx, person)?;
            let mut flags = flags_in(tx, person)?;
            let now = self.now();
            for &(flag, value) in changes {
                if flags.get(flag) == value {
                    continue;
                }
                tx.execute(
                    "INSERT INTO person_flags (person, flag, value, set_by, set_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT (person, flag) DO UPDATE SET
                       value = excluded.value, set_by = excluded.set_by, set_at = excluded.set_at",
                    params![person, flag.as_str(), value, set_by(ctx), now],
                )?;
                flags.set(flag, value);
                let rec = AuditRecord::new(AuditKind::AdminFlag, now.clone())
                    .with("person", person)
                    .with("flag", flag.as_str())
                    .with("value", value);
                self.record_audit(tx, ctx, rec)?;
            }
            Ok(flags)
        })
    }

    /// [`Store::set_flags`] for one flag named `name`, checked.
    ///
    /// # Errors
    /// `unknown_flag` for a name no flag has; [`Store::set_flags`]'.
    pub fn set_flag(&self, ctx: &AuditCtx, person: &str, name: &str, value: bool) -> Result<Flags> {
        let flag = Flag::parse(name)?;
        self.set_flags(ctx, person, &[(flag, value)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roles::tests::person;
    use crate::test_util::{DAEMON, store};

    #[test]
    fn defaults_are_on_for_the_owner_alone() {
        let (_d, st) = store();
        let sam = person(&st, "Sam", 1);
        assert_eq!(
            st.flags_of(OWNER).unwrap(),
            Flags {
                claxbot: true,
                claxbot_unenforced: false,
                feedback: false
            }
        );
        assert_eq!(
            st.flags_of(&sam).unwrap(),
            Flags {
                claxbot: false,
                claxbot_unenforced: false,
                feedback: false
            }
        );
    }

    #[test]
    fn setting_flags_records_each_change_once() {
        let (_d, st) = store();
        let sam = person(&st, "Sam", 1);
        let f = st
            .set_flags(
                DAEMON,
                &sam,
                &[(Flag::Claxbot, true), (Flag::ClaxbotUnenforced, false)],
            )
            .unwrap();
        assert!(f.claxbot && !f.claxbot_unenforced);
        // The same values again change nothing.
        st.set_flag(DAEMON, &sam, "claxbot", true).unwrap();
        let f = st.set_flag(DAEMON, OWNER, "claxbot", false).unwrap();
        assert!(!f.claxbot);
        assert!(!st.flags_of(OWNER).unwrap().claxbot);
        let events: Vec<String> = st
            .events_after(0, 100)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "admin.flag")
            .map(|e| e.body)
            .collect();
        assert_eq!(events.len(), 2, "{events:?}");
        assert!(events[0].contains("\"value\":true"));
        assert!(events[1].contains("\"person\":\"*owner\""));
    }

    #[test]
    fn feedback_flag_parses_and_defaults_off() {
        let (_d, st) = store();
        let sam = person(&st, "Sam", 1);
        assert_eq!(Flag::parse("feedback").unwrap(), Flag::Feedback);
        assert_eq!(Flag::Feedback.as_str(), "feedback");
        assert!(Flag::ALL.contains(&Flag::Feedback));
        for p in [OWNER, sam.as_str()] {
            assert!(!Flag::Feedback.default_for(p), "{p}");
            assert!(!st.flags_of(p).unwrap().feedback, "{p}");
        }
        let f = st.set_flag(DAEMON, &sam, "feedback", true).unwrap();
        assert!(f.feedback && f.get(Flag::Feedback) && !f.claxbot);
        assert!(!st.flags_of(OWNER).unwrap().feedback);
        let Err(CoreError::Invalid { message, .. }) = Flag::parse("nope") else {
            panic!("an unknown name is refused");
        };
        assert!(message.contains("feedback"), "{message}");
    }

    #[test]
    fn unknown_flags_and_people_are_refused() {
        let (_d, st) = store();
        assert!(matches!(
            st.set_flag(DAEMON, OWNER, "admin", true),
            Err(CoreError::Invalid {
                code: "unknown_flag",
                ..
            })
        ));
        assert!(matches!(
            st.set_flags(DAEMON, "u_nobody", &[(Flag::Claxbot, true)]),
            Err(CoreError::NotFound)
        ));
    }
}
