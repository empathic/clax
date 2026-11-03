//! A person's own choice about ClaxBot's web tools (migration 63, spec
//! 2026-11-02-claxbot-web-tools §9.2): web search and web fetch are on for
//! a person unless they turn them off, whatever the instance's
//! `web.*.enabled` settings say; the instance settings still decide
//! whether the tools exist at all. The choice is a row of
//! `bot_person_prefs`, kept per person key.

use super::Store;
use clax_core::Result;
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord};
use rusqlite::{OptionalExtension, params};

impl Store {
    /// Whether `person` allows ClaxBot the web tools: `true` when they
    /// never chose.
    ///
    /// # Errors
    /// The store's.
    pub fn web_choice(&self, person: &str) -> Result<bool> {
        self.with_read(|c| {
            Ok(
                c.prepare_cached("SELECT web FROM bot_person_prefs WHERE person = ?1")?
                    .query_row(params![person], |r| r.get::<_, bool>(0))
                    .optional()?
                    .unwrap_or(true),
            )
        })
    }

    /// Sets whether `person` allows ClaxBot the web tools, as `ctx`; a
    /// change is recorded as `me.web {person, value}`. Returns whether the
    /// choice changed.
    ///
    /// # Errors
    /// The store's.
    pub fn set_web_choice(&self, ctx: &AuditCtx, person: &str, on: bool) -> Result<bool> {
        self.with_tx(|tx| {
            let was: bool = tx
                .query_row(
                    "SELECT web FROM bot_person_prefs WHERE person = ?1",
                    params![person],
                    |r| r.get(0),
                )
                .optional()?
                .unwrap_or(true);
            if was == on {
                return Ok(false);
            }
            tx.execute(
                "INSERT INTO bot_person_prefs (person, web, set_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT (person) DO UPDATE SET web = excluded.web, set_at = excluded.set_at",
                params![person, on, self.now()],
            )?;
            let rec = AuditRecord::new(AuditKind::MeWeb, self.now())
                .with("person", person)
                .with("value", on);
            self.record_audit(tx, ctx, rec)?;
            Ok(true)
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::roles::OWNER;
    use crate::test_util::{DAEMON, store};

    #[test]
    fn a_person_chooses_web_off_and_back() {
        let (_d, st) = store();
        assert!(st.web_choice(OWNER).unwrap());
        assert!(!st.set_web_choice(DAEMON, OWNER, true).unwrap());
        assert!(st.set_web_choice(DAEMON, OWNER, false).unwrap());
        assert!(!st.web_choice(OWNER).unwrap());
        assert!(!st.set_web_choice(DAEMON, OWNER, false).unwrap());
        let events: Vec<String> = st
            .events_after(0, 100)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "me.web")
            .map(|e| e.body)
            .collect();
        assert_eq!(events.len(), 1, "one change, one event");
        assert!(events[0].contains("\"value\":false"), "{}", events[0]);
        assert!(st.set_web_choice(DAEMON, OWNER, true).unwrap());
        assert!(st.web_choice(OWNER).unwrap());
        assert!(st.web_choice("someone-else").unwrap());
    }
}
