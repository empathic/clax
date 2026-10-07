//! Browser viewers, keyed by the `clax_viewer` cookie (a ULID, the
//! viewer's credential) and named to others by a public ID.

use super::Store;
use crate::audit::{Actor, AuditCtx, AuditKind, AuditRecord, Via, sha256_hex};
use crate::gitctx::GitField;
use crate::ids::{is_public_id, is_ulid, new_public_id};
use crate::model::Viewer;
use crate::{CoreError, Result};
use rusqlite::{OptionalExtension, params};

/// Longest accepted display name, in characters.
pub const MAX_NAME_CHARS: usize = 60;

/// Most named viewers one [`Store::search_viewers`] call reads. The match runs
/// in Rust (SQLite folds ASCII case only), so a search costs time linear in the
/// names it reads; past this many it stops, and names later in name order are
/// not found by that search.
pub const MAX_SEARCH_SCAN: usize = 10_000;

const VIEWER_SELECT: &str = "SELECT id, public_id, display_name, created_at FROM viewers";

fn row_to_viewer(r: &rusqlite::Row<'_>) -> rusqlite::Result<Viewer> {
    Ok(Viewer {
        id: r.get(0)?,
        public_id: r.get(1)?,
        display_name: r.get(2)?,
        created_at: r.get(3)?,
    })
}

/// Checks a display name: at most [`MAX_NAME_CHARS`] characters, no control
/// characters. Returns it trimmed, `None` for an empty one.
fn checked_name(name: &str) -> Result<Option<&str>> {
    let n = name.trim();
    if n.chars().any(char::is_control) || n.chars().count() > MAX_NAME_CHARS {
        return Err(CoreError::invalid(
            "invalid_name",
            format!(
                "a display name is at most {MAX_NAME_CHARS} characters with no control characters"
            ),
        ));
    }
    Ok(Some(n).filter(|n| !n.is_empty()))
}

impl Store {
    /// Creates viewer `id` when missing, with a new public ID
    /// ([`crate::new_public_id`]) that it keeps for good. `display_name`: `None` keeps the
    /// current name, `Some("")` (after trimming) clears it, any other value
    /// replaces it. The owner's row is never reached this way (a viewer cookie
    /// never names it).
    ///
    /// # Errors
    /// `invalid_viewer` when `id` is not a ULID or names the owner's row;
    /// `invalid_name` for a name with control characters or longer than
    /// [`MAX_NAME_CHARS`].
    pub fn upsert_viewer(&self, id: &str, display_name: Option<&str>) -> Result<Viewer> {
        self.upsert_minted(id, display_name, None)
    }

    /// [`Store::upsert_viewer`] for a cookie the daemon is minting now, on a
    /// request from this machine (`local`: a loopback peer naming a literal
    /// local host) or not. Only a viewer minted locally may later be claimed
    /// for the owner ([`Store::claim_for_owner`]).
    pub fn mint_viewer(&self, id: &str, local: bool) -> Result<Viewer> {
        self.upsert_minted(id, None, Some(local))
    }

    fn upsert_minted(
        &self,
        id: &str,
        display_name: Option<&str>,
        local: Option<bool>,
    ) -> Result<Viewer> {
        if !is_ulid(id) {
            return Err(CoreError::invalid("invalid_viewer", "viewer IDs are ULIDs"));
        }
        let name = display_name.map(checked_name).transpose()?;
        self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO viewers (id, public_id, display_name, created_at, minted_local) VALUES (?1, ?2, ?3, ?4, ?6)
                 ON CONFLICT(id) DO UPDATE SET display_name = CASE WHEN ?5 THEN excluded.display_name ELSE display_name END
                 WHERE owner = 0",
                params![id, new_public_id(), name.flatten(), Store::now(), name.is_some(), local],
            )?;
            tx.query_row(&format!("{VIEWER_SELECT} WHERE id = ?1 AND owner = 0"), params![id], row_to_viewer)
                .optional()?
                .ok_or_else(|| CoreError::invalid("invalid_viewer", "that cookie names no viewer"))
        })
    }

    /// The viewer whose cookie is `id`; never the owner's row.
    pub fn get_viewer(&self, id: &str) -> Result<Option<Viewer>> {
        self.with_read(|c| {
            Ok(c.query_row(
                &format!("{VIEWER_SELECT} WHERE id = ?1 AND owner = 0"),
                params![id],
                row_to_viewer,
            )
            .optional()?)
        })
    }

    /// The viewer whose public ID is `public_id`; `None` for anything else,
    /// including a cookie value.
    pub fn viewer_by_public_id(&self, public_id: &str) -> Result<Option<Viewer>> {
        if !is_public_id(public_id) {
            return Ok(None);
        }
        self.with_read(|c| {
            Ok(c.query_row(
                &format!("{VIEWER_SELECT} WHERE public_id = ?1"),
                params![public_id],
                row_to_viewer,
            )
            .optional()?)
        })
    }
}

/// What [`Store::claim_for_owner`] did with a browser's viewer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Claim {
    /// Nothing: the cookie names no viewer minted on this machine.
    Nothing,
    /// The viewer became the owner, keeping its public ID, name and history.
    /// `retired` is the public ID of an owner row that held no browser's
    /// identity yet (made for the CLI): its history moved to this viewer and
    /// that ID names no one from now on.
    Adopted { retired: Option<String> },
    /// The viewer was folded into the owner and removed; its public ID
    /// (given here) names no one from now on.
    Merged(String),
}

fn owner_row(c: &rusqlite::Connection) -> rusqlite::Result<Option<(Viewer, bool)>> {
    c.query_row(
        "SELECT id, public_id, display_name, created_at, claimed FROM viewers WHERE owner = 1",
        [],
        |r| Ok((row_to_viewer(r)?, r.get::<_, bool>(4)?)),
    )
    .optional()
}

/// Moves `from`'s history to `into` and deletes `from`: its comments'
/// author, its mentions, the threads it resolved, its seen marks (the higher
/// of the two, pruned to [`crate::changelog::MAX_SEEN_PER_VIEWER`]), its
/// looked-at marks (the later), and its private documents
/// (`data/users/<from>/...` move under `data/users/<into>/` with a new
/// version; one whose destination exists stays where it is).
fn fold(
    st: &Store,
    ctx: &AuditCtx,
    tx: &rusqlite::Transaction<'_>,
    from: &Viewer,
    into: &Viewer,
) -> Result<()> {
    let (old, new) = (from.public_id.as_str(), into.public_id.as_str());
    tx.execute(
        "UPDATE comments SET author_public_id = ?2 WHERE author_public_id = ?1",
        params![old, new],
    )?;
    tx.execute(
        "UPDATE OR IGNORE mentions SET public_id = ?2 WHERE public_id = ?1",
        params![old, new],
    )?;
    tx.execute("DELETE FROM mentions WHERE public_id = ?1", params![old])?;
    tx.execute(
        "UPDATE threads SET resolved_by = 'viewer:' || ?2 WHERE resolved_by = 'viewer:' || ?1",
        params![old, new],
    )?;
    tx.execute(
        "INSERT INTO viewer_seen (viewer_id, artifact_id, seen_n, updated_at)
         SELECT ?2, artifact_id, seen_n, updated_at FROM viewer_seen WHERE viewer_id = ?1 AND true
         ON CONFLICT (viewer_id, artifact_id) DO UPDATE SET
           seen_n = MAX(seen_n, excluded.seen_n), updated_at = MAX(updated_at, excluded.updated_at)",
        params![from.id, into.id],
    )?;
    tx.execute(
        "DELETE FROM viewer_seen WHERE viewer_id = ?1",
        params![from.id],
    )?;
    tx.execute(
        "DELETE FROM viewer_seen WHERE viewer_id = ?1 AND artifact_id NOT IN
           (SELECT artifact_id FROM viewer_seen WHERE viewer_id = ?1 ORDER BY updated_at DESC, rowid DESC LIMIT ?2)",
        params![into.id, crate::changelog::MAX_SEEN_PER_VIEWER as i64],
    )?;
    tx.execute(
        "INSERT INTO viewer_threads (viewer_id, thread_id, looked_at)
         SELECT ?2, thread_id, looked_at FROM viewer_threads WHERE viewer_id = ?1 AND true
         ON CONFLICT (viewer_id, thread_id) DO UPDATE SET looked_at = MAX(looked_at, excluded.looked_at)",
        params![from.id, into.id],
    )?;
    tx.execute(
        "DELETE FROM viewer_threads WHERE viewer_id = ?1",
        params![from.id],
    )?;
    move_private_docs(st, ctx, tx, old, new)?;
    tx.execute("DELETE FROM viewers WHERE id = ?1", params![from.id])?;
    Ok(())
}

/// Records `viewer.claim`: `from`'s public ID is retired in favour of `to`'s.
fn record_claim(
    st: &Store,
    tx: &rusqlite::Transaction<'_>,
    ctx: &AuditCtx,
    from: &Viewer,
    to: &Viewer,
) -> Result<()> {
    let rec = AuditRecord::new(AuditKind::ViewerClaim, Store::now())
        .with("from_public_id", from.public_id.as_str())
        .with("to_public_id", to.public_id.as_str());
    st.record_audit(tx, ctx, rec)?;
    Ok(())
}

/// Moves every `data/users/<old>/...` document to `data/users/<new>/...`,
/// each with the next version of its artifact; a document whose destination
/// exists is left in place (and logged), and the destination kept. Each move
/// is recorded as `doc.move` under `ctx`, with both paths and the document's
/// hash, never its content.
fn move_private_docs(
    st: &Store,
    ctx: &AuditCtx,
    tx: &rusqlite::Transaction<'_>,
    old: &str,
    new: &str,
) -> Result<()> {
    let from = format!("data/users/{old}/");
    let to = format!("data/users/{new}/");
    let rows: Vec<(String, String, String)> = {
        let mut stmt = tx.prepare(
            "SELECT artifact_id, path, json FROM docs WHERE substr(path, 1, length(?1)) = ?1",
        )?;
        stmt.query_map(params![from], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    for (aid, path, json) in rows {
        let dest = format!("{to}{}", &path[from.len()..]);
        let taken: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM docs WHERE artifact_id = ?1 AND path = ?2)",
            params![aid, dest],
            |r| r.get(0),
        )?;
        if taken {
            tracing::warn!(artifact = %aid, %path, %dest, "a private document stays at its old path: the owner already has one there");
            continue;
        }
        let dp = crate::db::doc_path(&dest)?;
        let version: i64 = tx.query_row(
            "UPDATE artifacts SET doc_seq = doc_seq + 1 WHERE id = ?1 RETURNING doc_seq",
            params![aid],
            |r| r.get(0),
        )?;
        let now = Store::now();
        let version: i64 = tx.query_row(
            "UPDATE docs SET path = ?3, collection = ?4, version = MAX(version + 1, ?5), updated_at = ?6
             WHERE artifact_id = ?1 AND path = ?2 RETURNING version",
            params![aid, path, dest, dp.collection, version, now],
            |r| r.get(0),
        )?;
        let mut rec = AuditRecord::new(AuditKind::DocMove, now.as_str())
            .with("from", path.as_str())
            .with("to", dest.as_str())
            .with("collection", dp.collection.as_str())
            .with("doc_id", dp.id.as_str())
            .with("version", version)
            .with("sha256", format!("sha256:{}", sha256_hex(json.as_bytes())));
        rec.ids.artifact = Some(aid.clone());
        st.record_audit(tx, ctx, rec)?;
        tx.execute(
            "DELETE FROM leases WHERE artifact_id = ?1 AND path = ?2",
            params![aid, path],
        )?;
    }
    Ok(())
}

/// Gives viewer row `id` a fresh private ID (its cookie value), so a cookie
/// that named it before names nothing from now on; its seen and looked-at
/// marks follow it. Returns the new ID.
fn rotate_id(tx: &rusqlite::Transaction<'_>, id: &str) -> Result<String> {
    let fresh = crate::new_ulid();
    // The marks reference the row; the checks run at commit, once all three moved.
    tx.execute_batch("PRAGMA defer_foreign_keys = ON")?;
    tx.execute(
        "UPDATE viewers SET id = ?2 WHERE id = ?1",
        params![id, fresh],
    )?;
    tx.execute(
        "UPDATE viewer_seen SET viewer_id = ?2 WHERE viewer_id = ?1",
        params![id, fresh],
    )?;
    tx.execute(
        "UPDATE viewer_threads SET viewer_id = ?2 WHERE viewer_id = ?1",
        params![id, fresh],
    )?;
    Ok(fresh)
}

impl Store {
    /// The owner identity, when it exists: the one viewer that every owner
    /// credential (the bearer token, the owner and events cookies of the
    /// owner's browsers) acts as. Its `id` is never handed to anyone as a
    /// cookie.
    pub fn owner(&self) -> Result<Option<Viewer>> {
        self.with_read(|c| Ok(owner_row(c)?.map(|(v, _)| v)))
    }

    /// The owner identity, made when missing, with a new public ID and no
    /// name. `browser`: a browser of the owner's asks; its row then holds a
    /// browser's identity (a later claim folds into it, see
    /// [`Store::claim_for_owner`]). Without it (the CLI acting before any
    /// browser) a row made here gives way to the first browser claimed.
    pub fn owner_viewer(&self, browser: bool) -> Result<Viewer> {
        if let Some((v, claimed)) = self.with_read(|c| Ok(owner_row(c)?))?
            && (claimed || !browser)
        {
            return Ok(v);
        }
        self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO viewers (id, public_id, display_name, created_at, owner, claimed)
                 SELECT ?1, ?2, NULL, ?3, 1, ?4 WHERE NOT EXISTS (SELECT 1 FROM viewers WHERE owner = 1)",
                params![crate::new_ulid(), new_public_id(), Store::now(), browser],
            )?;
            if browser {
                tx.execute("UPDATE viewers SET claimed = 1 WHERE owner = 1", [])?;
            }
            Ok(owner_row(tx)?.expect("the owner row exists").0)
        })
    }

    /// Sets the owner's display name (made when missing, as for the CLI:
    /// see [`Store::owner_viewer`]); `""` clears it. `browser` as there.
    ///
    /// # Errors
    /// `invalid_name` as for [`Store::upsert_viewer`].
    pub fn set_owner_name(&self, name: &str, browser: bool) -> Result<Viewer> {
        let name = checked_name(name)?;
        let v = self.owner_viewer(browser)?;
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE viewers SET display_name = ?2 WHERE id = ?1",
                params![v.id, name],
            )?;
            Ok(owner_row(tx)?.expect("the owner row exists").0)
        })
    }

    /// Claims the viewer behind `cookie` for the owner, on the shell's token
    /// request (an owner credential, so the cookie's browser is the owner's).
    /// Only a viewer the daemon minted for a request from this machine
    /// qualifies: a cookie another page planted, a LAN viewer's, or one from
    /// before such origins were recorded claims nothing.
    ///
    /// With no owner yet, or one that holds no browser's identity (made for
    /// the CLI), the viewer becomes the owner, keeping its public ID, name
    /// and history ([`Claim::Adopted`]; such an earlier owner row is folded
    /// into it, its name kept when it has one). Otherwise the viewer is folded
    /// into the owner, which keeps its name when it has one
    /// ([`Claim::Merged`]). Either way the owner's private ID is new, so the
    /// cookie names nothing afterwards. Each private document that follows
    /// a folded viewer to the owner's public ID is recorded as `doc.move`,
    /// and a claim that retires a public ID (the folded row's) as one
    /// `viewer.claim {from_public_id, to_public_id}`, so records naming the
    /// retired ID resolve to its successor; all made by the owner (as it is
    /// after the claim) through `via`, in the claim's transaction.
    pub fn claim_for_owner(&self, via: Via, cookie: &str) -> Result<Claim> {
        let by = |owner: &Viewer| AuditCtx {
            actor: Actor::Owner {
                public_id: owner.public_id.clone(),
            },
            via,
            git: GitField::Absent,
            call: None,
        };
        self.with_tx(|tx| {
            let Some(legacy) = tx
                .query_row(
                    &format!("{VIEWER_SELECT} WHERE id = ?1 AND owner = 0 AND minted_local = 1"),
                    params![cookie],
                    row_to_viewer,
                )
                .optional()?
            else {
                return Ok(Claim::Nothing);
            };
            match owner_row(tx)? {
                Some((owner, true)) => {
                    fold(self, &by(&owner), tx, &legacy, &owner)?;
                    record_claim(self, tx, &by(&owner), &legacy, &owner)?;
                    if owner.display_name.is_none() && legacy.display_name.is_some() {
                        tx.execute(
                            "UPDATE viewers SET display_name = ?2 WHERE id = ?1",
                            params![owner.id, legacy.display_name],
                        )?;
                    }
                    Ok(Claim::Merged(legacy.public_id))
                }
                earlier => {
                    let retired = match earlier {
                        Some((cli, _)) => {
                            fold(self, &by(&legacy), tx, &cli, &legacy)?;
                            record_claim(self, tx, &by(&legacy), &cli, &legacy)?;
                            if cli.display_name.is_some() {
                                tx.execute(
                                    "UPDATE viewers SET display_name = ?2 WHERE id = ?1",
                                    params![legacy.id, cli.display_name],
                                )?;
                            }
                            Some(cli.public_id)
                        }
                        None => None,
                    };
                    tx.execute(
                        "UPDATE viewers SET owner = 1, claimed = 1 WHERE id = ?1",
                        params![legacy.id],
                    )?;
                    rotate_id(tx, &legacy.id)?;
                    Ok(Claim::Adopted { retired })
                }
            }
        })
    }
}

impl Store {
    /// The viewers with these public IDs, in the order given; unknown IDs,
    /// and anything that is not a public ID, are skipped.
    pub fn viewers_by_public_ids(&self, ids: &[String]) -> Result<Vec<Viewer>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(&format!("{VIEWER_SELECT} WHERE public_id = ?1"))?;
            let mut out = Vec::new();
            for id in ids.iter().filter(|i| is_public_id(i)) {
                if let Some(v) = stmt.query_row(params![id], row_to_viewer).optional()? {
                    out.push(v);
                }
            }
            Ok(out)
        })
    }

    /// Up to `limit` named viewers whose name contains `q` as literal text,
    /// ignoring case in every script (Unicode lowercase on both sides),
    /// ordered by name (then public ID). Reads at most [`MAX_SEARCH_SCAN`]
    /// named viewers, in that order: its cost is linear in the names read.
    pub fn search_viewers(&self, q: &str, limit: usize) -> Result<Vec<Viewer>> {
        let needle = q.to_lowercase();
        self.with_read(|c| {
            // SQLite's lower() folds ASCII only, so names are matched here,
            // streaming in name order and stopping at `limit` hits or
            // MAX_SEARCH_SCAN names read.
            let mut stmt = c.prepare(&format!(
                "{VIEWER_SELECT} WHERE display_name IS NOT NULL
                 ORDER BY display_name COLLATE NOCASE, public_id"
            ))?;
            let mut out = Vec::new();
            for v in stmt.query_map([], row_to_viewer)?.take(MAX_SEARCH_SCAN) {
                if out.len() >= limit {
                    break;
                }
                let v = v?;
                if v.display_name
                    .as_deref()
                    .is_some_and(|n| n.to_lowercase().contains(&needle))
                {
                    out.push(v);
                }
            }
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Claim, MAX_NAME_CHARS, MAX_SEARCH_SCAN};
    use crate::audit::Via;
    use crate::store::test_util::{artifact, store};
    use crate::{CoreError, new_ulid};

    #[test]
    fn the_owner_is_one_viewer_no_cookie_reaches() {
        let (_d, st) = store();
        assert_eq!(st.owner().unwrap(), None, "made on first use only");
        let a = st.owner_viewer(false).unwrap();
        assert_eq!(st.owner_viewer(true).unwrap(), a);
        assert_eq!(st.owner().unwrap().as_ref(), Some(&a));
        assert_eq!(
            st.get_viewer(&a.id).unwrap(),
            None,
            "a cookie never names the owner"
        );
        assert!(matches!(
            st.upsert_viewer(&a.id, Some("Mallory")),
            Err(CoreError::Invalid {
                code: "invalid_viewer",
                ..
            })
        ));
        assert_eq!(
            st.set_owner_name(" Alex ", true)
                .unwrap()
                .display_name
                .as_deref(),
            Some("Alex")
        );
        assert_eq!(st.owner().unwrap().unwrap().public_id, a.public_id);
    }

    fn private_doc(st: &crate::Store, aid: &crate::ArtifactId, path: &str, json: &str) {
        st.with_write(|c| {
            c.execute(
                "INSERT INTO docs (artifact_id, path, collection, json, version, updated_at) VALUES (?1, ?2, ?3, ?4, 1, 'x')",
                rusqlite::params![aid.as_str(), path, crate::db::doc_path(path).unwrap().collection, json],
            )?;
            Ok(())
        })
        .unwrap();
    }

    fn doc_json(st: &crate::Store, aid: &crate::ArtifactId, path: &str) -> Option<String> {
        use rusqlite::OptionalExtension;
        st.with_read(|c| {
            Ok(c.query_row(
                "SELECT json FROM docs WHERE artifact_id = ?1 AND path = ?2",
                rusqlite::params![aid.as_str(), path],
                |r| r.get(0),
            )
            .optional()?)
        })
        .unwrap()
    }

    #[test]
    fn only_a_viewer_minted_on_this_machine_is_claimed() {
        let (_d, st) = store();
        // Before origins were recorded, from elsewhere (a LAN viewer, whose
        // cookie another page could plant), or unknown: never the owner.
        let unknown = st.upsert_viewer(&new_ulid(), Some("Old")).unwrap();
        let lan = new_ulid();
        st.mint_viewer(&lan, false).unwrap();
        for c in [
            unknown.id.as_str(),
            lan.as_str(),
            "01J9Z3K4M5N6P7Q8R9S0T1V2W3",
        ] {
            assert_eq!(
                st.claim_for_owner(Via::Shell, c).unwrap(),
                Claim::Nothing,
                "{c}"
            );
        }
        assert_eq!(st.owner().unwrap(), None);
        assert!(
            st.get_viewer(&lan).unwrap().is_some(),
            "the LAN viewer is untouched"
        );
        assert!(claims(&st).is_empty());
    }

    #[test]
    fn the_first_claimed_viewer_becomes_the_owner_under_a_new_private_id() {
        let (_d, st) = store();
        let cookie = new_ulid();
        let chrome = st.mint_viewer(&cookie, true).unwrap();
        st.upsert_viewer(&cookie, Some("Alex")).unwrap();
        assert_eq!(
            st.claim_for_owner(Via::Shell, &cookie).unwrap(),
            Claim::Adopted { retired: None }
        );
        let owner = st.owner().unwrap().unwrap();
        assert_eq!(
            (owner.public_id.as_str(), owner.display_name.as_deref()),
            (chrome.public_id.as_str(), Some("Alex")),
            "the adopted viewer keeps its public ID and name"
        );
        assert_ne!(owner.id, cookie, "the old cookie no longer names the row");
        assert_eq!(st.get_viewer(&cookie).unwrap(), None);
        assert!(st.upsert_viewer(&cookie, Some("x")).unwrap().public_id != owner.public_id);
        assert_eq!(
            st.claim_for_owner(Via::Shell, &cookie).unwrap(),
            Claim::Nothing,
            "a fresh row is not local"
        );
        assert!(claims(&st).is_empty(), "no public ID was retired");
    }

    #[test]
    fn a_browser_claimed_after_the_cli_keeps_its_ids_and_takes_the_clis_history() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let cli = st.set_owner_name("Alex", false).unwrap();
        st.create_thread(&aid, thread_by(&cli.public_id, "from the CLI"))
            .unwrap();
        private_doc(
            &st,
            &aid,
            &format!("data/users/{}/prefs", cli.public_id),
            "{\"cli\":1}",
        );
        let cookie = new_ulid();
        let chrome = st.mint_viewer(&cookie, true).unwrap();
        assert_eq!(
            st.claim_for_owner(Via::Shell, &cookie).unwrap(),
            Claim::Adopted {
                retired: Some(cli.public_id.clone())
            }
        );
        let owner = st.owner().unwrap().unwrap();
        assert_eq!(
            owner.public_id, chrome.public_id,
            "the browser's user ID stays"
        );
        assert_eq!(
            owner.display_name.as_deref(),
            Some("Alex"),
            "the name set in the CLI stays"
        );
        assert_eq!(st.viewer_by_public_id(&cli.public_id).unwrap(), None);
        let author: String = st
            .with_read(|c| {
                Ok(c.query_row("SELECT author_public_id FROM comments", [], |r| r.get(0))?)
            })
            .unwrap();
        assert_eq!(author, chrome.public_id);
        assert_eq!(
            doc_json(&st, &aid, &format!("data/users/{}/prefs", chrome.public_id)).as_deref(),
            Some("{\"cli\":1}")
        );
        // The move is the owner's, through the shell, and names no content.
        let moves = doc_moves(&st);
        assert_eq!(moves.len(), 1);
        let (actor, artifact, body) = &moves[0];
        assert_eq!(
            actor,
            &serde_json::json!({"type": "owner", "public_id": chrome.public_id})
        );
        assert_eq!(artifact.as_deref(), Some(aid.as_str()));
        assert_eq!(body["via"], "shell");
        assert_eq!(body["from"], format!("data/users/{}/prefs", cli.public_id));
        assert_eq!(body["to"], format!("data/users/{}/prefs", chrome.public_id));
        assert_eq!(
            body["sha256"],
            format!("sha256:{}", crate::audit::sha256_hex(b"{\"cli\":1}"))
        );
        // The CLI's retired public ID resolves to the browser's.
        assert_eq!(
            claims(&st),
            vec![(
                chrome.public_id.clone(),
                cli.public_id.clone(),
                chrome.public_id.clone()
            )]
        );
        assert!(!body.to_string().contains("\\\"cli\\\""), "{body}");
    }

    /// The events of `kind`: actor, artifact ID and body.
    fn events_of(
        st: &crate::Store,
        kind: &str,
    ) -> Vec<(serde_json::Value, Option<String>, serde_json::Value)> {
        st.events_after(0, 100)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == kind)
            .map(|e| {
                (
                    serde_json::from_str(&e.actor).unwrap(),
                    e.ids.artifact,
                    serde_json::from_str(&e.body).unwrap(),
                )
            })
            .collect()
    }

    fn doc_moves(st: &crate::Store) -> Vec<(serde_json::Value, Option<String>, serde_json::Value)> {
        events_of(st, "doc.move")
    }

    /// The `viewer.claim` events as (actor's public ID, from, to).
    fn claims(st: &crate::Store) -> Vec<(String, String, String)> {
        events_of(st, "viewer.claim")
            .into_iter()
            .map(|(actor, _, b)| {
                let s = |v: &serde_json::Value| v.as_str().unwrap().to_string();
                (
                    s(&actor["public_id"]),
                    s(&b["from_public_id"]),
                    s(&b["to_public_id"]),
                )
            })
            .collect()
    }

    fn thread_by(public_id: &str, body: &str) -> crate::store::threads::NewThread {
        crate::store::threads::NewThread {
            author_public_id: Some(public_id.to_string()),
            version_n: 1,
            anchor: crate::store::test_util::anchor(),
            author_name: "Alex S".into(),
            body: body.into(),
            clip: None,
            via_page: false,
        }
    }

    #[test]
    fn a_later_claimed_viewer_merges_into_the_owner() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let owner = st.owner_viewer(true).unwrap();
        let cookie = new_ulid();
        let safari = st.mint_viewer(&cookie, true).unwrap();
        st.upsert_viewer(&cookie, Some("Alex S")).unwrap();
        let t = st
            .create_thread(&aid, thread_by(&safari.public_id, "hi"))
            .unwrap();
        st.resolve_thread(&t.id, &format!("viewer:{}", safari.public_id))
            .unwrap();
        st.mark_seen(&safari.id, &aid, 1).unwrap();
        st.mark_looked(&safari.id, &aid, std::slice::from_ref(&t.id))
            .unwrap();
        st.with_write(|c| {
            c.execute(
                "INSERT INTO mentions (comment_id, public_id) SELECT id, ?1 FROM comments",
                rusqlite::params![safari.public_id],
            )?;
            Ok(())
        })
        .unwrap();
        let (old, new) = (
            format!("data/users/{}/", safari.public_id),
            format!("data/users/{}/", owner.public_id),
        );
        private_doc(&st, &aid, &format!("{old}profile"), "{\"pick\":3}");
        private_doc(&st, &aid, &format!("{old}prefs"), "{\"theme\":\"safari\"}");
        private_doc(&st, &aid, &format!("{new}prefs"), "{\"theme\":\"owner\"}");
        assert_eq!(
            st.claim_for_owner(Via::Shell, &cookie).unwrap(),
            Claim::Merged(safari.public_id.clone())
        );
        assert_eq!(st.get_viewer(&cookie).unwrap(), None, "the row is gone");
        let now = st.owner().unwrap().unwrap();
        assert_eq!(now.public_id, owner.public_id);
        assert_eq!(
            now.display_name.as_deref(),
            Some("Alex S"),
            "an unnamed owner takes the name"
        );
        assert_eq!(st.seen(&now.id, &aid).unwrap(), Some(1));
        let (author, resolved_by, mention, looked): (String, String, String, i64) = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT (SELECT author_public_id FROM comments), (SELECT resolved_by FROM threads),
                            (SELECT public_id FROM mentions),
                            (SELECT COUNT(*) FROM viewer_threads WHERE viewer_id = ?1)",
                    rusqlite::params![now.id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )?)
            })
            .unwrap();
        assert_eq!(author, owner.public_id);
        assert_eq!(resolved_by, format!("viewer:{}", owner.public_id));
        assert_eq!(mention, owner.public_id);
        assert_eq!(looked, 1);
        // Private documents move; on a conflict the owner's stays and the old one is left.
        assert_eq!(
            doc_json(&st, &aid, &format!("{new}profile")).as_deref(),
            Some("{\"pick\":3}")
        );
        assert_eq!(doc_json(&st, &aid, &format!("{old}profile")), None);
        assert_eq!(
            doc_json(&st, &aid, &format!("{new}prefs")).as_deref(),
            Some("{\"theme\":\"owner\"}")
        );
        assert_eq!(
            doc_json(&st, &aid, &format!("{old}prefs")).as_deref(),
            Some("{\"theme\":\"safari\"}")
        );
        // The merged viewer's public ID resolves to the owner's.
        assert_eq!(
            claims(&st),
            vec![(
                owner.public_id.clone(),
                safari.public_id.clone(),
                owner.public_id.clone()
            )]
        );
        // Only the document that moved is recorded, as the owner's.
        let moves = doc_moves(&st);
        assert_eq!(moves.len(), 1);
        assert_eq!(moves[0].0["public_id"], owner.public_id.as_str());
        assert_eq!(
            (&moves[0].2["from"], &moves[0].2["to"]),
            (
                &serde_json::json!(format!("{old}profile")),
                &serde_json::json!(format!("{new}profile"))
            )
        );
        // A named owner keeps its name.
        let laptop = new_ulid();
        st.mint_viewer(&laptop, true).unwrap();
        st.upsert_viewer(&laptop, Some("Other")).unwrap();
        st.claim_for_owner(Via::Shell, &laptop).unwrap();
        assert_eq!(
            st.owner().unwrap().unwrap().display_name.as_deref(),
            Some("Alex S")
        );
    }

    #[test]
    fn upsert_creates_renames_and_clears() {
        let (_d, st) = store();
        let id = new_ulid();
        let v = st.upsert_viewer(&id, None).unwrap();
        assert_eq!(v.display_name, None);
        assert_eq!(
            st.upsert_viewer(&id, Some("  Alex  "))
                .unwrap()
                .display_name
                .as_deref(),
            Some("Alex")
        );
        assert_eq!(
            st.upsert_viewer(&id, None).unwrap().display_name.as_deref(),
            Some("Alex"),
            "None keeps the name"
        );
        assert_eq!(
            st.upsert_viewer(&id, Some("")).unwrap().display_name,
            None,
            "empty clears"
        );
        assert_eq!(
            st.get_viewer(&id).unwrap().unwrap().created_at,
            v.created_at
        );
    }

    #[test]
    fn a_viewer_has_a_stable_public_id_distinct_from_its_cookie() {
        let (_d, st) = store();
        let id = new_ulid();
        let v = st.upsert_viewer(&id, None).unwrap();
        assert!(crate::is_public_id(&v.public_id), "{}", v.public_id);
        assert_ne!(v.public_id, id);
        assert_eq!(
            st.upsert_viewer(&id, Some("Alex")).unwrap().public_id,
            v.public_id
        );
        let other = st.upsert_viewer(&new_ulid(), None).unwrap();
        assert_ne!(other.public_id, v.public_id);
        let found = st.viewer_by_public_id(&v.public_id).unwrap().unwrap();
        assert_eq!(
            (found.id.as_str(), found.display_name.as_deref()),
            (id.as_str(), Some("Alex"))
        );
        assert_eq!(
            st.viewer_by_public_id("u_0123456789abcdef012345").unwrap(),
            None
        );
        assert_eq!(
            st.viewer_by_public_id(&id).unwrap(),
            None,
            "the cookie is not a public ID"
        );
        let json = serde_json::to_value(&v).unwrap();
        assert!(
            !json.to_string().contains(&id),
            "a serialised viewer never carries its cookie: {json}"
        );
        assert_eq!(json["public_id"], v.public_id.as_str());
    }

    #[test]
    fn bad_ids_and_names_are_refused() {
        let (_d, st) = store();
        assert!(matches!(
            st.upsert_viewer("not-a-ulid", None),
            Err(CoreError::Invalid {
                code: "invalid_viewer",
                ..
            })
        ));
        let id = new_ulid();
        for bad in ["a\nb".to_string(), "x".repeat(MAX_NAME_CHARS + 1)] {
            assert!(matches!(
                st.upsert_viewer(&id, Some(&bad)),
                Err(CoreError::Invalid {
                    code: "invalid_name",
                    ..
                })
            ));
        }
    }

    #[test]
    fn lookups_by_public_id_and_name_search() {
        let (_d, st) = store();
        let alex = st.upsert_viewer(&new_ulid(), Some("Alex Chen")).unwrap();
        let sam = st.upsert_viewer(&new_ulid(), Some("Sam")).unwrap();
        let anon = st.upsert_viewer(&new_ulid(), None).unwrap();
        let found: Vec<_> = st
            .viewers_by_public_ids(&[
                anon.public_id.clone(),
                "u_ffffffffffffffffffffff".into(),
                alex.public_id.clone(),
                "not a public ID".into(),
            ])
            .unwrap()
            .into_iter()
            .map(|v| v.public_id)
            .collect();
        assert_eq!(
            found,
            [anon.public_id.clone(), alex.public_id.clone()],
            "in the order asked, unknown and malformed IDs skipped"
        );
        let names: Vec<_> = st
            .search_viewers("A", 8)
            .unwrap()
            .into_iter()
            .map(|v| v.display_name.unwrap())
            .collect();
        assert_eq!(
            names,
            ["Alex Chen", "Sam"],
            "case-insensitive substring, by name, named only"
        );
        assert_eq!(
            st.search_viewers("chen", 8).unwrap()[0].public_id,
            alex.public_id
        );
        assert!(st.search_viewers("zz", 8).unwrap().is_empty());
        assert_eq!(st.search_viewers("a", 1).unwrap().len(), 1);
        let _ = sam;
    }

    #[test]
    fn search_folds_case_beyond_ascii_and_matches_wildcards_literally() {
        let (_d, st) = store();
        let umlaut = st.upsert_viewer(&new_ulid(), Some("Ärger")).unwrap();
        st.upsert_viewer(&new_ulid(), Some("Arno")).unwrap();
        let pct = st.upsert_viewer(&new_ulid(), Some("100% sure")).unwrap();
        st.upsert_viewer(&new_ulid(), Some("100 x sure")).unwrap();
        let under = st.upsert_viewer(&new_ulid(), Some("a_b")).unwrap();
        st.upsert_viewer(&new_ulid(), Some("axb")).unwrap();
        let ids = |q: &str| -> Vec<String> {
            st.search_viewers(q, 8)
                .unwrap()
                .into_iter()
                .map(|v| v.public_id)
                .collect()
        };
        assert_eq!(ids("ärger"), std::slice::from_ref(&umlaut.public_id));
        assert_eq!(ids("ÄRGER"), std::slice::from_ref(&umlaut.public_id));
        assert_eq!(ids("0%"), std::slice::from_ref(&pct.public_id));
        assert_eq!(ids("a_b"), std::slice::from_ref(&under.public_id));
        assert!(ids("%").len() == 1 && ids("_").len() == 1);
    }
    #[test]
    fn search_scans_at_most_max_search_scan_names() {
        assert_eq!(MAX_SEARCH_SCAN, 10_000);
        let (_d, st) = store();
        // MAX_SEARCH_SCAN names sorting before one more, "zed", which lies past the scan.
        st.with_write(|c| {
            let tx = c.unchecked_transaction()?;
            {
                let mut ins = tx.prepare(
                    "INSERT INTO viewers (id, public_id, display_name, created_at)
                     VALUES (?1, 'u_' || lower(hex(randomblob(11))), ?2, '2026-01-01T00:00:00Z')",
                )?;
                for i in 0..=MAX_SEARCH_SCAN {
                    let name = if i == MAX_SEARCH_SCAN {
                        "zed".to_string()
                    } else {
                        format!("a{i:05}")
                    };
                    ins.execute(rusqlite::params![format!("v{i}"), name])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            st.search_viewers("a09999", 8).unwrap().len(),
            1,
            "the last scanned name is found"
        );
        assert!(
            st.search_viewers("zed", 8).unwrap().is_empty(),
            "a name past the scan is not reached"
        );
    }
}
