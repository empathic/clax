/**
 * Clax's additions to runtime contract 0.2.61.
 *
 * The files under `0.2.61/` are claude.ai's type definitions, unchanged.
 * What Clax answers beyond them is declared here, so a page written for
 * claude.ai keeps working and a page written for Clax can handle the extra
 * cases. Served at `<daemon_url>/_clax/contract/clax-extensions.d.ts`.
 */
declare namespace ClaxExtensions {
  /**
   * An extra error code of the calls that act beyond a composer the viewer
   * sees: `artifact.publish` (`artifact.d.ts`), and `create`, `reply`,
   * `resolve`, `delete` and `sendToClaude` (`comments.d.ts`).
   *
   * - `shell_input_recent` — the call came from the viewer's click or key
   *   in the page, but within 5.5 s of their input to the Clax window
   *   around it (its name field, composer or buttons), so nothing was
   *   published or written. Keep any draft and tell the viewer to click
   *   again. It is not charged to any budget. A page that treats unknown
   *   codes as the contract says (`upstream_error`) still behaves
   *   correctly.
   */
  type ShellInputRecent = "shell_input_recent";

  /** The codes Clax adds to `ArtifactErrorCode` (`artifact.d.ts`). */
  type ArtifactErrorCode = ShellInputRecent;

  /**
   * The codes Clax adds to `CommentsErrorCode` (`comments.d.ts`).
   *
   * Clax also answers `unavailable` from `create`, `reply`, `resolve` and
   * `delete` called without the viewer's own click or key in the page
   * (`sendToClaude` answers `claude_unavailable`, as the contract says).
   * One retry from a fresh gesture is reasonable.
   */
  type CommentsErrorCode = ShellInputRecent;

  /** One agent session working on this artifact now (`comments.working()`). */
  interface WorkingAgent {
    /** `"claude"`, `"codex"`, `"pi"`, or another harness name. */
    harness: string;
    /** The harness as people read it ("Claude Code"). */
    label: string;
    /** The agent's own words, at most 140 characters; treat as untrusted text. */
    message: string | null;
    /** When it started (ISO 8601). */
    since: string;
    /** Handles of threads THIS document created that the agent is acting on. */
    threads: string[];
    /** How many other threads it is acting on. */
    otherThreads: number;
  }

  /** What `working()` resolves and `onWorking` reports. */
  interface WorkingState {
    working: boolean;
    /** Newest first. */
    agents: WorkingAgent[];
  }

  /**
   * Members Clax adds to the `comments` namespace (`comments.d.ts`
   * `interface Comments`). Granted under either declaration form, including
   * `composer_only` (which otherwise grants only openComposer and
   * anchorFor), with no consent prompt and no gesture. They never name a
   * session or a thread's store ID.
   */
  interface Comments {
    /** Which agents are working on this artifact now. Read-only. */
    working(): Promise<WorkingState>;
    /** Calls `fn` with the current state, then on every change. Resolves a function that stops the calls. */
    onWorking(fn: (state: WorkingState) => void): Promise<() => void>;
  }

  /**
   * The codes `db.artifact(id)` adds to `DbErrorCode` (`db.d.ts`), on calls
   * and on the `onSnapshot` error callback:
   *
   * - `not_declared` — this artifact's `capabilities.db.read` does not list
   *   `id` (declare it: `{"db": {"read": ["<id>"]}}`), or app `id` does not
   *   declare `db` and so keeps no documents to share. Nothing was asked.
   * - `not_granted` — the owner said no (Clax asks once per page load), or
   *   a viewer who is not the owner has no grant to use. Render without that
   *   app's data; retrying in this page load cannot succeed.
   *
   * When the owner withdraws a grant while the page runs, a live listener
   * stops receiving that app's changes at once; its error callback gets
   * `not_granted` at the listener's next refresh (after a reconnect of the
   * event stream), and any later `get()` rejects `not_granted`.
   */
  type DbCrossReadErrorCode = "not_declared" | "not_granted";

  /** A document of another app: read and follow, never write. */
  interface DbReadOnlyDocument {
    id: string;
    path: string;
    get(): Promise<DocumentSnapshot>;
    onSnapshot(next: (snap: DocumentSnapshot) => void, error?: (e: DbError) => void): Unsubscribe;
    collection(path: string): DbReadOnlyCollection;
  }

  /** A query over another app's collection: the builders of `Query`. */
  interface DbReadOnlyQuery {
    where(field: string, op: string, value: unknown): DbReadOnlyQuery;
    orderBy(field: string, dir?: "asc" | "desc"): DbReadOnlyQuery;
    limit(n: number): DbReadOnlyQuery;
    get(): Promise<QuerySnapshot>;
    onSnapshot(next: (snap: QuerySnapshot) => void, error?: (e: DbError) => void): Unsubscribe;
  }

  /** A collection of another app (no `add`). */
  interface DbReadOnlyCollection extends DbReadOnlyQuery {
    path: string;
    doc(id: string): DbReadOnlyDocument;
  }

  /** Another app's documents, read-only. */
  interface DbReadOnly {
    doc(path: string): DbReadOnlyDocument;
    collection(path: string): DbReadOnlyCollection;
  }

  /**
   * Members Clax adds to the `db` namespace (`db.d.ts` `type DB`).
   *
   * Another app's documents are readable when this artifact declares the
   * read in `capabilities.db.read` (`{"db": {"read": ["<id>"]}}`, at most 16
   * artifact IDs) and either that app shares with this one
   * (`{"db": {"share": {"read": ["<this ID>"]}}}`) or the owner allows it:
   * Clax asks the owner on the first read ("Let … read …'s data?") and keeps
   * the answer. The reads are judged at `view`: documents private to one
   * viewer (`data/users/<id>/…`) and paths whose rules need more than
   * `view` behave as missing.
   */
  interface Db {
    /** Read-only refs over the documents of app `id` (an artifact ID).
     * Throws `TypeError` for a value that is not one; calls reject with
     * {@link DbCrossReadErrorCode}s. */
    artifact(id: string): DbReadOnly;
  }
}
