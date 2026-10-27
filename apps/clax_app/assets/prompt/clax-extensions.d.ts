/**
 * Clax's additions to runtime contract 0.2.61.
 *
 * The files under `0.2.61/` are claude.ai's type definitions, unchanged.
 * Where Clax departs from them it says so here: `assets.d.ts` has a page
 * rebuild an asset's URL as `"/_blob/" + id`, but Clax serves assets per
 * artifact, so a page uses `claude.assetUrl(id)` (below).
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
     * - `system_denied` (location): location is off for Clax in the
     *   system's settings (Android's, or the Mac's System Settings) and the
     *   system will not ask again; Clax offers the person Open settings.
     *   Say that location is off for Clax.
     * - `location_off` (location): the device's location is off; Clax
     *   offers Open settings.
     * - `stopped` (`location.watch`): the watch ended because the app left
     *   the screen (or the person stopped it); start it again from a tap.
     * - `dismissed`: the person closed the question without answering;
     *   nothing is kept, and the next call (from a tap) asks again.
     */
    type ErrorCode = "not_granted" | "cancelled" | "unavailable" | "bad_request" | "too_large" | "no_gesture" | "system_denied" | "location_off" | "stopped" | "dismissed";

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

    /** Where the device is: degrees, and `accuracy` in metres. `precise`
     * is false when the person allowed the app only Approximate location
     * (or the system allows Clax only that): then the position is the centre
     * of an area about 2 km across, `accuracy` is at least 2000, and it
     * stays the same anywhere in that area. */
    interface Position {
      lat: number;
      lon: number;
      accuracy: number;
      precise: boolean;
    }

    interface WatchOptions {
      /** Metres the device moves between updates: 10 when absent, 5 to
       * 10000. */
      minDistance?: number;
      /** Called once when the watch ends without `stop()`: `stopped` (the
       * app left the screen, or the person stopped location; it never
       * starts again by itself), or a refused or failed start
       * (`not_granted`, `system_denied`, `location_off`, `unavailable`,
       * `no_gesture`, `bad_request`). No update follows it. */
      onError?(e: { code: ErrorCode; message: string }): void;
    }

    interface Location {
      /** Where the device is now. */
      current(): Promise<Position>;
      /** Follows the device while the app is on screen: `onUpdate` gets a
       * position at once, then at most once a second and once per
       * `minDistance`. An approximate one reports a new area only after
       * the device has stayed well inside it for minutes, at most every
       * 10 minutes. Call it from a click handler: one tap starts one
       * location call, and after a watch ends a new tap is needed. At
       * most 4 at once. Clax shows a location pill while it runs. Returns
       * `stop`. */
      watch(onUpdate: (p: Position) => void, opts?: WatchOptions): () => void;
    }
  }

  /**
   * The home screen's capability, served by the Clax app to one app only:
   * the one it shows as its home screen. Declare it (`{"home": {}}`) and
   * obtain it with `await claude.use("home")`: declaring grants nothing,
   * and it resolves `null` in a browser, in the draft preview and in every
   * other app, so render a plain page that says to open it in the Clax app
   * first. The home app itself is never in `apps()`.
   *
   * Calls that open something (`open`, `openMake`, `newApp`, `menu`,
   * `openInbox`, `openSettings`, `importApp`, `getApps`) must come from
   * the person's tap or key press in the page, or they reject
   * `no_gesture`; the reads, `chrome`, `shown` and `tiles` need none.
   *
   * Clax keeps its own home screen over the page until the page calls
   * `shown()`, and shows its own, with Undo last change and Reset, when
   * no `shown()` comes within 10 s of the page loading. Rejections are `{code, message}` with `code` one of
   * `refused` (not the home app), `bad_request` (an ID not on the home
   * screen, a malformed rect, bad menu items), `no_gesture` or
   * `unavailable`.
   */
  namespace Home {
    /** An app's icon: its own (`url`, an image URL to show in an `<img>`),
     * an emoji, an icon word (`chart`, `calendar`, `recipe`, …), or one or
     * two letters on a hue (0 to 359) of the app's own. */
    type Icon = { url: string } | { emoji: string } | { word: string } | { monogram: string; hue: number };

    interface Progress {
      done: number;
      total: number;
    }

    interface App {
      id: string;
      title: string;
      /** ISO 8601. */
      updatedAt: string;
      icon: Icon;
      /** How far its build is, while one goes. */
      build: Progress | null;
      /**
       * How a published app stands against its public copy, for its tile's
       * badge: live (☁), ahead (↑, newer here), update (↓, an update
       * waiting; the only warm badge), diverged (⑂, changed in both
       * places) or fork (⑂, a copy installed from someone else and changed
       * here); null when it is not published or the person kept their
       * changes. Absent from a Clax app older than this field: read it as
       * null.
       */
      sync?: "live" | "ahead" | "update" | "diverged" | "fork" | null;
    }

    interface Make {
      id: string;
      /** What the person asked for. */
      request: string;
      state: "making" | "offline" | "waiting" | "interrupted" | "failed";
      /** The features its plan has done, once the plan has any. */
      progress: Progress | null;
    }

    /** A tile's place on the page, in CSS pixels from the viewport's top left. */
    interface Rect {
      x: number;
      y: number;
      w: number;
      h: number;
    }

    /** The window the home screen is in, as `chrome()` answers it. */
    interface Chrome {
      /** The system Clax runs on. */
      platform: "android" | "macos" | "ios";
      /** `phone` on a narrow window (below 600 dp), `wide` on a tablet,
       * a foldable opened, a Chromebook or a desktop window. */
      form: "phone" | "wide";
      /** Whether the window's own toolbar carries the top bar, search and
       * New app, so the page draws none of them. */
      toolbar: boolean;
      /** The pointer in use: say "Click" for `mouse` (a mouse or a
       * trackpad), "Tap" for `touch`. */
      pointer: "touch" | "mouse";
      /** With `toolbar`, the text in the toolbar's search field (`""`
       * while it is empty): search the apps for it as for the page's own
       * field; each change comes as a `chrome` event. Missing where the
       * window has no toolbar search. */
      query?: string;
      /** `true` where `getApps()` shows Get my apps; offer it only then.
       * Missing where this Clax offers no Get my apps. */
      getApps?: boolean;
    }

    /** What `on(name)` hears: each topic's whole latest value. */
    interface Events {
      apps: App[];
      makes: Make[];
      unread: number;
      key: boolean;
      chrome: Chrome;
    }

    /** An item a home app adds to an app's menu. */
    interface MenuItem {
      /** What `menu` resolves with when the person picks it: 1 to 64 characters, each item its own. */
      id: string;
      /** What the menu shows: plain text, 1 to 32 characters. */
      label: string;
    }

    interface Home {
      /** The person's apps, newest first. */
      apps(): Promise<App[]>;
      /** The makes going on, waiting, interrupted or failed, newest first. */
      makes(): Promise<Make[]>;
      /** The inbox's unread count. */
      unread(): Promise<number>;
      /** Whether the Claude API key is missing (no app can be made). */
      needsKey(): Promise<boolean>;
      /** Opens app `id`, as a tap on its tile. */
      open(id: string): Promise<void>;
      /** Shows make `id`'s build view, or asks to make an interrupted one again. */
      openMake(id: string): Promise<void>;
      /** Shows the New app sheet; the make then appears in `makes()`. */
      newApp(): Promise<void>;
      /** Shows app `id`'s menu anchored at `rect`: the page's `items` (at
       * most 4) first, above a divider, then Clax's own (Open, Inspect,
       * Share app, Resume build). Resolves with the picked item's `id`, or
       * `null` when the person picked one of Clax's own or dismissed it.
       * Bad items reject `bad_request`. */
      menu(id: string, rect: Rect, options?: { items?: MenuItem[] }): Promise<string | null>;
      openInbox(): Promise<void>;
      openSettings(): Promise<void>;
      /** Opens the system picker to import a `.clax` app. */
      importApp(): Promise<void>;
      /** Shows Get my apps: the apps of the account the Clax app is signed
       * in to on a hosted Clax, to bring down, signing in there first
       * when it is not. Offer it only where
       * `chrome().getApps` is `true`. */
      getApps(): Promise<void>;
      /** Says the page has drawn the home screen: the grid with its apps,
       * the empty state or the load error. Call it once per load, after
       * the first such render (from an effect, not a tap). Resolves
       * `{opened: id}` when Clax opened app `id` from a tap on the image of
       * the home screen it showed until then (an open from the home screen,
       * as a tap on the tile), else `null`. */
      shown(): Promise<{ opened: string } | null>;
      /** Says where each app's tile is now, after each render that moves
       * them (and on scroll and resize): Clax keeps the last ones with an
       * image of the page, which it shows at once on its next start, and
       * opens the app under a tap on that image once the page is shown. At
       * most 512 tiles. */
      tiles(tiles: { id: string; rect: Rect }[]): Promise<void>;
      /** The window the page is in, and the pointer in use; `on("chrome")`
       * hears each change. An older Clax rejects it (`unavailable`, or it
       * is missing): treat that as `{form: "phone", toolbar: false,
       * pointer: "touch"}`. */
      chrome(): Promise<Chrome>;
      /** Calls `fn` with each change of topic `name`, after the page's
       * first `home` call; returns `stop`. A page that was covered hears
       * only the latest value. */
      on<K extends keyof Events>(name: K, fn: (value: Events[K]) => void): () => void;
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
      /** The body's bytes, exactly as the server sent them (an image, say). */
      arrayBuffer(): Promise<ArrayBuffer>;
      /** The body's bytes as a `Blob` of the response's `content-type`; `URL.createObjectURL` shows it in an `<img>`. */
      blob(): Promise<Blob>;
    }

    interface Net {
      fetch(url: string, init?: RequestInit): Promise<Response>;
    }
  }

  /**
   * Clax's own controls around the page, for a page that walks its viewer
   * through them (a tour). Declare `{"shell": {}}` (it takes no options)
   * and obtain it with `await claude.use("shell")`: it is served in the
   * owner's and editors' views, and resolves `null` for every other viewer
   * and when undeclared, so render without it first.
   *
   * - `highlight(control)` rings that control for 6 s or until the viewer
   *   presses anywhere in Clax's own bar or panels or presses Escape there
   *   (a press in the page does not reach Clax: call `highlight` again to
   *   move the ring, or let it lapse; the control's own click acts as
   *   usual), one ring at a time: the next `highlight` moves it. It
   *   resolves `true` once drawn, and `false`, drawing nothing, when the
   *   control is not shown to this viewer now: at phone width `versions`,
   *   `publish` and `fork` are in the more menu, and `send` needs the
   *   threads shown. Clax opens no menu, panel or tab to reach a control.
   *   No gesture is needed.
   * - `can(control)` answers what `highlight` would, without drawing.
   * - `commentMode(on)` turns comment mode on or off, only from a click or key press in the page (while comment mode is on, a click in the page picks what to comment on and never reaches the page, so leave it from a key press; the viewer's Escape or Comment button also leaves it), and resolves whether comment mode is on
   *   afterwards: leaving keeps it on while the viewer's comment holds
   *   typed text.
   * - `address()` resolves where this page is in Clax: `origin`, the Clax
   *   that frames it (`https://clax.dev`, `http://localhost:7480`), and
   *   `url`, this artifact's address there (`<origin>/a/<ID>`, the latest
   *   version), the one to give an agent to watch. It resolves `null` in
   *   the Clax app. The page cannot learn the origin otherwise: its site
   *   sends no referrer, so `document.referrer` is empty.
   *
   * It says only whether a named control is shown, and the Clax and
   * address the page is already shown at; nothing about who else is here,
   * which agent works on the artifact, or any credential.
   */
  namespace Shell {
    /** `comment`: the Comment button. `threads`: the Threads button.
     * `versions`: the version menu's button. `publish`: Publish (or the
     * linked site's button). `fork`: Fork. `send`: the first Send to agent
     * in the threads sidebar. */
    type Control = "comment" | "threads" | "versions" | "publish" | "fork" | "send";

    /**
     * Rejection codes (as `{code, message}`):
     * - `invalid_argument`: a control outside `Control`, or `on` not a boolean.
     * - `no_gesture`: `commentMode` without the viewer's recent click or key
     *   press in the page; nothing changes.
     * - `unavailable`: `commentMode` where this view cannot comment (a live
     *   page, a crowd's spectator, Comments off, an agent's draft shown, the
     *   Clax app's inspector).
     */
    type ErrorCode = "invalid_argument" | "no_gesture" | "unavailable";

    interface Shell {
      highlight(control: Control): Promise<boolean>;
      commentMode(on: boolean): Promise<boolean>;
      can(control: Control): Promise<boolean>;
      address(): Promise<Address | null>;
    }

    /** `address()`'s answer: the framing Clax's origin, and this artifact's address there. */
    interface Address {
      origin: string;
      url: string;
    }
  }
}

/** What Clax adds to `window.claude` (`claude.d.ts` `interface Claude`). */
interface Claude {
  /**
   * The URL of this artifact's asset `id` (as `assets.upload` resolved it
   * and `db` keeps it): `/_blob/<artifact id>/<asset id>`, the same URL
   * `assets` gives, resolving from every page and version of the artifact
   * and for every viewer, `assets` granted or not. Assets are served per
   * artifact and a page loads only its own, so never build `"/_blob/" +
   * id`. A value that is not an asset ID gives a URL that answers 404.
   */
  assetUrl(id: string): string;
}
