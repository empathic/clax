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

  /**
   * Clax's device capabilities, served only in the Clax app (Android; on
   * macOS what the platform allows, else `unavailable`). Declare each one
   * used (`{"notify": {}, "camera": {}}`) and obtain it with
   * `await claude.use("notify")`: it resolves `null` in a browser, in the
   * app's draft preview, and when undeclared, so render without it first.
   * The person is asked once per app and capability, on its first call,
   * and the answer (Allow or Don't allow) stands until they revoke it in
   * the app inspector.
   *
   * Every call except `notify.list` and `notify.cancel` must come from the
   * person's tap or key press in the page (a click handler), or it rejects
   * `no_gesture`.
   *
   * Binary data travels as base64 strings (`data`), at most 4 MB a file.
   */
  namespace Device {
    /**
     * Rejection codes of every device call (as `{code, message}`):
     * - `not_granted`: the person did not allow this capability for the
     *   app (or the system permission is off); hide or explain the feature.
     *   Calling again does not ask again.
     * - `cancelled`: the person closed the picker, sheet or camera.
     * - `unavailable`: this device cannot do it (no camera, location off, a
     *   platform without it), or the view has closed.
     * - `bad_request`: arguments of the wrong shape (a bug in the page).
     * - `too_large`: a file over 4 MB (`file.pick`, `share.file`,
     *   `file.save`).
     * - `no_gesture`: called outside the person's tap or key press in the
     *   page; call it from a click handler.
     */
    type ErrorCode = "not_granted" | "cancelled" | "unavailable" | "bad_request" | "too_large" | "no_gesture";

    /** A file's bytes, as base64. */
    interface FileData {
      name: string;
      mime: string;
      data: string;
    }

    /** A reminder: once `at` (ISO 8601 date-time, in the future), or
     * repeating `every` day at `hour:minute` local time, on `weekdays` only
     * when given (1 Monday … 7 Sunday). `route` (`#…`) is where a tap on it
     * opens the app; the app's start page when absent. Scheduling an `id`
     * the app already scheduled replaces it; an app keeps at most 64. Delivery may be a few minutes
     * late: the device batches alarms to save battery. */
    interface Reminder {
      /** The page's own name for it, 1 to 64 characters. */
      id: string;
      title: string;
      body?: string;
      at?: string;
      every?: { hour: number; minute: number; weekdays?: number[] };
      route?: string;
    }

    interface Notify {
      schedule(r: Reminder): Promise<{ id: string }>;
      cancel(id: string): Promise<{ cancelled: boolean }>;
      /** The app's reminders still to come (one-off ones not yet shown). */
      list(): Promise<Reminder[]>;
    }

    interface Share {
      /** Opens the system share sheet; resolves once the person chose. */
      text(text: string): Promise<{ shared: boolean }>;
      file(f: FileData): Promise<{ shared: boolean }>;
    }

    /** The `file` capability. */
    interface FilePicker {
      /** One file the person picks; `accept` is a list of extensions
       * (`["csv", "json"]`) or MIME types (`["image/*"]`). */
      pick(opts?: { accept?: string[] }): Promise<FileData>;
      /** Lets the person save `f` where they choose. */
      save(f: FileData): Promise<{ saved: boolean }>;
    }

    /** A photo, JPEG, downscaled so neither side exceeds 1600 px. */
    interface Photo {
      mime: string;
      data: string;
      width: number;
      height: number;
    }

    interface Camera {
      /** Takes a photo with the camera. */
      take(): Promise<Photo>;
      /** Picks one from the person's photos. */
      pick(): Promise<Photo>;
    }

    interface Location {
      /** Where the device is now: degrees, and accuracy in metres. */
      current(): Promise<{ lat: number; lon: number; accuracy: number }>;
    }
  }

  /**
   * The network, through the Clax daemon (the page itself may not connect:
   * its content security policy keeps `connect-src 'none'`). Served to the
   * owner (in the Clax app, or the owner's browser). Declare the hosts the
   * app talks to, `{"net": {"hosts": ["api.example.com"]}}` (lowercase
   * host names, no scheme, port or wildcard; at most 32), and obtain it
   * with `await claude.use("net")`: it resolves `null` when undeclared or
   * for another viewer, so render without it first.
   *
   * The owner is asked once per app, on the first fetch, for the whole host
   * list; a host a later version adds is asked again. The answer stands
   * until revoked in the app inspector. Requests go only to `https` URLs on
   * a declared host whose every address is public (never loopback or a
   * private network); redirects (at most 3) follow the same rules. Bodies
   * are at most 5 MB each way, and a fetch takes at most 20 s. Cookies are
   * neither sent nor kept.
   */
  namespace Net {
    /**
     * Rejection codes of `fetch` (as `{code, message}`):
     * - `not_declared`: the URL's host is not in `net.hosts`.
     * - `not_granted`: the owner did not allow this app's hosts; calling
     *   again in this view does not ask again.
     * - `https_only`: the URL (or a redirect) is not `https`.
     * - `redirect_refused`: a redirect to a host the app may not reach.
     * - `blocked_address`: the host resolves to a non-public address.
     * - `too_large`: a body over 5 MB.
     * - `too_many_redirects`, `timeout`, `upstream_error`: the server could
     *   not be reached or did not answer in time. `upstream_error` is not
     *   an HTTP error status: those resolve with their `status`.
     * - `invalid_argument`: a malformed URL, method or header.
     * - `unavailable`, `session_expired`: the daemon could not be reached,
     *   or the page must be reloaded.
     */
    type ErrorCode = "not_declared" | "not_granted" | "https_only" | "redirect_refused" | "blocked_address" | "too_large" | "too_many_redirects" | "timeout" | "upstream_error" | "invalid_argument" | "unavailable" | "session_expired";

    interface RequestInit {
      /** `GET` when absent. */
      method?: string;
      /** `Cookie`, `Host` and connection headers are dropped. */
      headers?: Record<string, string>;
      /** Text; `JSON.stringify` an object and set `content-type`. */
      body?: string;
    }

    interface Response {
      /** The HTTP status; 4xx and 5xx resolve too. */
      status: number;
      /** `status` is 200 to 299. */
      ok: boolean;
      /** The final URL, after redirects. */
      url: string;
      /** Lowercase names; no `set-cookie`. */
      headers: Record<string, string>;
      /** The body as text (UTF-8). */
      text(): Promise<string>;
      /** The body parsed as JSON; rejects (a `SyntaxError`) when it is not JSON. */
      json<T = unknown>(): Promise<T>;
    }

    interface Net {
      fetch(url: string, init?: RequestInit): Promise<Response>;
    }
  }
}
