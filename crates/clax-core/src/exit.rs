//! The statuses a daemon run ends with, shared by `clax serve` and the
//! core a host app runs in its own process (`clax-embed`).

/// The status a run ends with when its database's schema could not be
/// brought up to date ([`crate::CoreError::MigrationFailed`]): a failure
/// that repeats on every start until the database or the binary changes,
/// so a supervisor stops restarting on it, and the embedded core tells its
/// host app the open failed this way rather than for a reason a retry
/// could fix.
pub const MIGRATION_FAILED: i32 = 65;

/// The status for a run that failed with `e`: [`MIGRATION_FAILED`] when
/// `e` or any error in its `source()` chain is a failed migration,
/// otherwise 1.
pub fn status_of(e: &(dyn std::error::Error + 'static)) -> i32 {
    let mut cur = Some(e);
    while let Some(c) = cur {
        if matches!(
            c.downcast_ref::<crate::CoreError>(),
            Some(crate::CoreError::MigrationFailed { .. })
        ) {
            return MIGRATION_FAILED;
        }
        cur = c.source();
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CoreError;
    use std::error::Error;
    use std::fmt;

    /// A layer of context over an inner error, as `anyhow`'s `context`
    /// adds one: its `source()` is the inner error.
    #[derive(Debug)]
    struct Ctx(Box<dyn Error + 'static>);

    impl fmt::Display for Ctx {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("starting the daemon")
        }
    }

    impl Error for Ctx {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(self.0.as_ref())
        }
    }

    #[test]
    fn a_failed_migration_ends_with_its_own_status() {
        let e = Ctx(Box::new(CoreError::MigrationFailed {
            at: 27,
            detail: "duplicate column name: draft_of".into(),
        }));
        assert_eq!(status_of(&e), MIGRATION_FAILED);
        assert_eq!(MIGRATION_FAILED, 65);
        assert_eq!(status_of(&std::io::Error::other("other")), 1);
        let newer = CoreError::SchemaNewer {
            found: 40,
            known: 31,
        };
        assert_eq!(status_of(&newer), 1);
    }
}
