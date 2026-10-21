//! A person agreeing to a hosted Clax's Terms of Service and Privacy Policy
//! (`legal_agreements`, migration 39): which version of the documents each
//! viewer row agreed to, when, and how. A new version of the documents
//! leaves the older agreements in place; whether a person has agreed is
//! asked of one version.

use super::Store;
use clax_core::Result;
use rusqlite::params;

/// How a person agreed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgreedVia {
    /// Signing a browser in under the sign-in page's notice.
    SignIn,
    /// The prompt a signed-in person is shown when they have not agreed to
    /// the current version.
    Prompt,
}

impl AgreedVia {
    pub fn as_str(self) -> &'static str {
        match self {
            AgreedVia::SignIn => "signin",
            AgreedVia::Prompt => "prompt",
        }
    }
}

impl Store {
    /// Records `viewer_id` agreeing to `version`, `via` how. Agreeing to a
    /// version again keeps the first agreement.
    pub fn legal_agree(&self, viewer_id: &str, version: &str, via: AgreedVia) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "INSERT OR IGNORE INTO legal_agreements (viewer_id, version, agreed_at, via)
                 VALUES (?1, ?2, ?3, ?4)",
                params![viewer_id, version, self.now(), via.as_str()],
            )?;
            Ok(())
        })
    }

    /// Whether `viewer_id` has agreed to `version`.
    pub fn legal_agreed(&self, viewer_id: &str, version: &str) -> Result<bool> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT EXISTS (SELECT 1 FROM legal_agreements WHERE viewer_id = ?1 AND version = ?2)",
                params![viewer_id, version],
                |r| r.get::<_, bool>(0),
            )?)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agreement_is_per_version_and_keeps_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&clax_core::Home::at(dir.path().join("ax"))).unwrap();
        assert!(!s.legal_agreed("v1", "2026-10-19").unwrap());
        s.legal_agree("v1", "2026-10-19", AgreedVia::SignIn)
            .unwrap();
        s.legal_agree("v1", "2026-10-19", AgreedVia::Prompt)
            .unwrap();
        assert!(s.legal_agreed("v1", "2026-10-19").unwrap());
        assert!(!s.legal_agreed("v1", "2027-01-01").unwrap());
        assert!(!s.legal_agreed("v2", "2026-10-19").unwrap());
        let via: String = s
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT via FROM legal_agreements WHERE viewer_id = 'v1'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(via, "signin");
    }
}
