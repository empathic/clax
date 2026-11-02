// Generated from web/sdk/openapi.json by web/scripts/contract-types.mjs: do not edit.
// The Clax API contract (spec 2026-10-27-clax-sdk-design §4.1), version 1.0.0.
// Regenerate with `just contract`.

/** The contract's version: its major is what `GET /.well-known/clax` serves as `contract_version`. */
export type ContractVersion = "1.0.0";

/**
 * Spec §9 "Anchors". Every field but `kind` and `file` may be null. `file` is
 * the published path of the page the anchor is on, `index.html` when absent;
 * an `artifact` anchor is on no page, and its `file` is empty.
 * An `area` anchor is a rectangle the viewer drew: `selector` names the
 * smallest element containing it, `area` places it within that element, and
 * `rect` holds it in viewport pixels at draw time; `area` is serialized only
 * when present.
 */
export interface Anchor {
  area?: AnchorArea;
  custom_name: string | null;
  file: string;
  html_hash: string | null;
  kind: AnchorKind;
  /**
   * The quote as a person reads it: its text with a space where it runs
   * from one element into another, which the quote (kept as the page's
   * text nodes join, for re-anchoring) lacks. Shown in place of the quote;
   * absent on anchors made without one, which show the quote.
   */
  label?: string;
  /**
   * What the element is called, as the bridge read it when it was picked:
   * its accessible name, else the start of its text, else its kind with
   * the nearest heading ("Chart under ‘p95 by four-hour window’"). Shown
   * as a thread's title where the anchor quotes nothing; absent on
   * anchors made without one.
   */
  name?: string;
  prefix: string | null;
  quote: string | null;
  rect: AnchorRect | null;
  /**
   * The route a live page's thread was made at (spec 2026-10-05 §5.2):
   * the query and hash route, set by the daemon; absent elsewhere.
   */
  route?: string;
  selector: string | null;
  suffix: string | null;
}

/**
 * A drawn rectangle as fractions (0 to 1) of its element's border box: `x`
 * and `y` from its top left corner, `w` and `h` of its width and height.
 * `tag`, `text`, and `children` fingerprint that element at draw time (its
 * local name, the first characters of its text with whitespace collapsed,
 * its child element count), so re-anchoring by selector alone can tell
 * another element at the same path; each is optional and serialized only
 * when present.
 */
export interface AnchorArea {
  children?: number;
  h: number;
  tag?: string;
  text?: string;
  w: number;
  x: number;
  y: number;
}

/**
 * A drawn rectangle as fractions (0 to 1) of its element's border box: `x`
 * and `y` from its top left corner, `w` and `h` of its width and height.
 * `tag`, `text`, and `children` fingerprint that element at draw time (its
 * local name, the first characters of its text with whitespace collapsed,
 * its child element count), so re-anchoring by selector alone can tell
 * another element at the same path; each is optional and serialized only
 * when present.
 */
export interface AnchorAreaInput {
  children?: number | null;
  h: number;
  tag?: string | null;
  text?: string | null;
  w: number;
  x: number;
  y: number;
}

/**
 * Spec §9 "Anchors". Every field but `kind` and `file` may be null. `file` is
 * the published path of the page the anchor is on, `index.html` when absent;
 * an `artifact` anchor is on no page, and its `file` is empty.
 * An `area` anchor is a rectangle the viewer drew: `selector` names the
 * smallest element containing it, `area` places it within that element, and
 * `rect` holds it in viewport pixels at draw time; `area` is serialized only
 * when present.
 */
export interface AnchorInput {
  area?: AnchorAreaInput | null;
  custom_name?: string | null;
  file?: string;
  html_hash?: string | null;
  kind: AnchorKind;
  /**
   * The quote as a person reads it: its text with a space where it runs
   * from one element into another, which the quote (kept as the page's
   * text nodes join, for re-anchoring) lacks. Shown in place of the quote;
   * absent on anchors made without one, which show the quote.
   */
  label?: string | null;
  /**
   * What the element is called, as the bridge read it when it was picked:
   * its accessible name, else the start of its text, else its kind with
   * the nearest heading ("Chart under ‘p95 by four-hour window’"). Shown
   * as a thread's title where the anchor quotes nothing; absent on
   * anchors made without one.
   */
  name?: string | null;
  prefix?: string | null;
  quote?: string | null;
  rect?: AnchorRect | null;
  /**
   * The route a live page's thread was made at (spec 2026-10-05 §5.2):
   * the query and hash route, set by the daemon; absent elsewhere.
   */
  route?: string | null;
  selector?: string | null;
  suffix?: string | null;
}

export type AnchorKind = "element" | "range" | "custom" | "area" | "artifact";

/**
 * The anchored region at pick time, in viewport pixels, with the page's
 * scroll offsets and viewport width.
 */
export interface AnchorRect {
  h: number;
  scrollX: number;
  scrollY: number;
  viewportW: number;
  w: number;
  x: number;
  y: number;
}

/** Every error answer's body (`clax_http::error::ApiError`). */
export interface ApiErrorBody {
  error: ApiErrorDetail;
}

/** An error answer's `error`. */
export interface ApiErrorDetail {
  /**
   * What went wrong, for code to branch on: one of the operation's
   * `x-clax-codes` for its status.
   */
  code: string;
  /** What went wrong, for a person. */
  message: string;
}

/**
 * What waits for a reporter: its reports with a fix waiting for its
 * verdict, or with a reply it has not read (`…/seen`), oldest first.
 */
export interface Badge {
  report_ids: string[];
  /** How many reports wait (`report_ids.len()`). */
  waiting: number;
}

/** How the report's clip was captured (§4.1). */
export interface CaptureInfo {
  method: CaptureMethod;
  mode: CaptureMode;
  /** What the render left out, and where (§4.2). */
  omitted?: Omitted[];
}

/**
 * How the clip was made (§4.1): `dom`, a render of the page's DOM, or
 * `display`, the browser's screen capture.
 */
export type CaptureMethod = "dom" | "display";

/** What the reporter captured (§4.1). */
export type CaptureMode = "element" | "range" | "area" | "screen";

/** A comment's body. */
export interface CommentRequest {
  body: string;
}

/** The paths of a device sign-in. */
export interface DevicePaths {
  approve: string;
  code: string;
  token: string;
}

/** A line of a report's diagnostics (§5.2). */
export interface DiagLine {
  /** When it happened, as the browser stamped it. */
  at: string;
  /**
   * One of [`DIAG_KINDS`]: `console`, `error` (an uncaught error),
   * `rejection` (an unhandled promise rejection) or `request` (a failed
   * request).
   */
  kind: "console" | "error" | "rejection" | "request";
  /** One of [`DIAG_LEVELS`]. */
  level: "debug" | "info" | "warn" | "error";
  /** What was logged, scrubbed. */
  text: string;
}

/** `{lines}`: a capture's diagnostics, `[]` when it has none. */
export interface DiagnosticsAnswer {
  lines: DiagLine[];
}

/** The answer of `GET /.well-known/clax`. */
export interface Discovery {
  /** Its API root: `/api`. */
  api: string;
  /** What it offers beyond the core. */
  capabilities: string[];
  /**
   * The major version of the contract it serves
   * (`clax_core::contract::MAJOR`).
   */
  contract_version: string;
  /** The paths of its device sign-in, when it has one. */
  device?: DevicePaths;
  /** `hosted` or `local` (`worker` for the hosted Worker). */
  kind: "local" | "hosted" | "worker";
  /** Its name. */
  name: string;
  /** Where it says it is reached. */
  origin: string;
  /** How a client obtains a credential there. */
  sign_in: string[];
  /** The domain its artifacts' sites are served under, when it has one. */
  site_domain?: string;
}

/**
 * The page and browser the capture was made in. `url` keeps its query
 * keys, never their values ([`scrub_url`]); `route` and `title` are
 * [`scrub`]bed.
 */
export interface Environment {
  /** `light` or `dark`, as the page's `prefers-color-scheme` matched. */
  color_scheme?: "light" | "dark";
  locale?: string;
  platform?: string;
  route?: string;
  scroll?: Scroll;
  time_zone?: string;
  title?: string;
  url?: string;
  user_agent?: string;
  viewport?: Viewport;
}

/**
 * The page and browser the capture was made in. `url` keeps its query
 * keys, never their values ([`scrub_url`]); `route` and `title` are
 * [`scrub`]bed.
 */
export interface EnvironmentInput {
  /** `light` or `dark`, as the page's `prefers-color-scheme` matched. */
  color_scheme?: "light" | "dark";
  locale?: string | null;
  platform?: string | null;
  route?: string | null;
  scroll?: Scroll | null;
  time_zone?: string | null;
  title?: string | null;
  url?: string | null;
  user_agent?: string | null;
  viewport?: Viewport | null;
}

/**
 * A fix proposed on a report (§6.2), numbered from 1 in the order
 * proposed, with its verdict once given (§6.3): the verdict's note, the
 * context of whoever gave it, and when.
 */
export interface FixAttempt {
  commit?: string;
  created_at: string;
  n: number;
  outcome: FixOutcome;
  /**
   * Whether an agent or a person proposed it. While it waits, the
   * agent's session hears the reporter's replies and may answer them;
   * a person's fix is answered from the triage views.
   */
  proposer_kind?: ProposerKind;
  proposer_name: string;
  /** The release the fix ships in. */
  release?: string;
  /** Plain text, posted to the report as a comment the reporter sees. */
  summary: string;
  /** The change: a pull request, a commit page, a ticket. */
  url?: string;
  verdict_at?: string;
  verdict_context?: ReportContext;
  verdict_note?: string;
  /** When the reporter can check it, such as `release:0.6.1`. */
  verify_after?: string;
  /** Where the reporter checks the fix. */
  verify_url?: string;
}

/** What became of a fix attempt (§6.3). */
export type FixOutcome = "pending" | "verified" | "rejected" | "superseded" | "closed";

/** A feature flag's value: a boolean, a number or a string. */
export type FlagValue = boolean | number | string;

/**
 * A claim's answer (§7.4): the lease, its expiry, its fence (larger for
 * each new lease on the report), and the report.
 */
export interface Lease {
  expires_at: string;
  fence: number;
  lease_id: string;
  report: Report;
}

/**
 * A part of the page a DOM render left out (§4.2): its kind (`iframe`,
 * `canvas`, `video`, a masked element, …) and its box.
 */
export interface Omitted {
  kind: string;
  rect: Rect;
}

/**
 * Who proposed a fix: an agent, through its session under its lease, or
 * a person (a triage member, in their browser).
 */
export type ProposerKind = "agent" | "person";

/** A queue as a member reads it, with the reader's own role. */
export interface Queue {
  created_at: string;
  id: string;
  name: string;
  /**
   * The deployer's app's origins (normalized by [`normalize_origin`]):
   * a reporter token is minted only for one of them.
   */
  origins: string[];
  reporters_see: ReportersSee;
  role: QueueRole;
}

/** `{queues}`: the caller's queues with its role on each. */
export interface QueueList {
  queues: Queue[];
}

/**
 * A member's right on a queue (§7.1): `admin` changes the queue and makes
 * service grants, and holds triage too; `triage` reads every report,
 * claims, replies, resolves and writes thread metadata.
 */
export type QueueRole = "triage" | "admin";

/** A box in CSS pixels. */
export interface Rect {
  h: number;
  w: number;
  x: number;
  y: number;
}

/** The release of the host app the reporter ran, as the host names it. */
export interface Release {
  channel?: string;
  commit?: string;
  id?: string;
  version?: string;
}

/** The release of the host app the reporter ran, as the host names it. */
export interface ReleaseInput {
  channel?: string | null;
  commit?: string | null;
  id?: string | null;
  version?: string | null;
}

/** A report: a thread filed through a queue (§7). */
export interface Report {
  anchor: Anchor;
  artifact: ReportArtifact;
  claim: ReportClaim | null;
  comments: ReportComment[];
  /**
   * What the reporter's browser said at capture (feedback-loop §5.1);
   * absent on a report filed without one, and on a report served to a
   * reporter that did not file it (`reporters_see: all`).
   */
  context?: ReportContext;
  created_at: string;
  /**
   * How many diagnostics lines its latest capture holding any has: the
   * filing's, or a later reopen's (feedback-loop §5.2); 0 when none
   * has.
   */
  diagnostics_lines?: number;
  /**
   * How many diagnostics lines the filing itself (capture 0) has; 0
   * when it came with none.
   */
  filing_diagnostics_lines?: number;
  /** The fixes proposed on it, oldest first (feedback-loop §6.2). */
  fixes?: FixAttempt[];
  has_clip: boolean;
  /** The thread's ID. */
  id: string;
  /** Opaque JSON the app wrote with the report; `null` when none. */
  metadata: unknown;
  metadata_version: number;
  queue_id: string;
  /**
   * How many times the reporter (or triage on their behalf) reopened
   * it (feedback-loop §6.3).
   */
  reopen_count?: number;
  /**
   * The reporter that filed it, from its reporter token; `None` for a
   * report filed otherwise.
   */
  reporter_id: string | null;
  /**
   * The reporter's profile (feedback-loop §5.4), served to triage
   * members only.
   */
  reporter_profile?: ReporterProfile;
  /**
   * Why a `verified` or `resolved` report was closed (feedback-loop
   * §6.1).
   */
  resolve_reason?: ResolveReason;
  /**
   * The receiving server's stamp (feedback-loop §5.1); absent on a
   * report filed without one.
   */
  server?: ServerStamp;
  state: ReportState;
  /** The thread's status: `open` or `resolved`. */
  status: "open" | "resolved";
}

/** `{report}`: the answer of a route that changes or files one report. */
export interface ReportAnswer {
  report: Report;
}

/**
 * The external artifact a report is on, as a report carries it (§6.1).
 * Its fields are shared by every queue whose reports it holds.
 */
export interface ReportArtifact {
  external_id: string;
  id: string;
  /** Opaque JSON the app wrote; `null` when it wrote none. */
  metadata: unknown;
  metadata_version: number;
  revision: string | null;
  title: string;
  url: string;
}

/** A live claim on a report: who holds it, until when. */
export interface ReportClaim {
  expires_at: string;
  holder_name: string;
}

/**
 * A comment of a report: `author_kind` is `viewer`, `agent` or
 * [`AUTHOR_REPORTER`]. A reopen's comment (feedback-loop §6.3) carries
 * its capture: the reopen's number, whether a clip came with it, and how
 * many diagnostics lines.
 */
export interface ReportComment {
  author_kind: "viewer" | "agent" | "reporter";
  author_name: string;
  body: string;
  /**
   * The reopen this comment was posted with (1 for the first); absent
   * on any other comment.
   */
  capture?: number;
  created_at: string;
  diagnostics_lines?: number;
  has_clip?: boolean;
  id: string;
  /**
   * A note for the queue's members alone (a person's release of a
   * claim): a reporter's view of the report leaves it out.
   */
  triage_only?: boolean;
}

/**
 * What the reporter's browser said at capture (§5.1): filled by the SDK
 * and the host app, checked by [`parse_context`], stored with the report
 * and never changed. Its top level admits no other key; the objects inside
 * it ignore keys they do not know.
 */
export interface ReportContext {
  /** Opaque JSON the host app adds; every string in it is scrubbed. */
  app?: unknown;
  capture?: CaptureInfo;
  /** When the capture was made, RFC 3339. */
  captured_at: string;
  environment?: Environment;
  /** The host app's feature flags at capture. */
  flags?: Record<string, FlagValue>;
  release?: Release;
  /** The shape's version: 1. */
  v: 1;
}

/**
 * What the reporter's browser said at capture (§5.1): filled by the SDK
 * and the host app, checked by [`parse_context`], stored with the report
 * and never changed. Its top level admits no other key; the objects inside
 * it ignore keys they do not know.
 */
export interface ReportContextInput {
  /** Opaque JSON the host app adds; every string in it is scrubbed. */
  app?: unknown;
  capture?: CaptureInfo | null;
  /** When the capture was made, RFC 3339. */
  captured_at: string;
  environment?: EnvironmentInput | null;
  /** The host app's feature flags at capture. */
  flags?: Record<string, FlagValue>;
  release?: ReleaseInput | null;
  /** The shape's version: 1. */
  v: 1;
}

/**
 * A page of reports; `next` is the cursor of the following page, `null`
 * on the last.
 */
export interface ReportPage {
  next: string | null;
  reports: Report[];
}

/**
 * Where a report stands (spec 2026-10-29-feedback-loop-design §6.1):
 * `open`, waiting for someone; `claimed`, while a live lease holds it;
 * `fix_proposed`, while a fix waits for the reporter's verdict;
 * `verified`, once the reporter said it is fixed; `resolved`, once triage
 * closed it without a verdict. `verified` and `resolved` resolve the
 * report's thread.
 */
export type ReportState = "open" | "claimed" | "fix_proposed" | "verified" | "resolved";

/**
 * Who a reporter is, as the integrator's backend asserts it when minting
 * the reporter token (§5.4). Shown to triage members, never to
 * reporters.
 */
export interface ReporterProfile {
  /** An `https:` or `http:` image URL. */
  avatar_url?: string;
  email?: string;
  /** A GitHub login, for asking the reporter to review a fix. */
  github?: string;
  name?: string;
}

/**
 * Who a reporter is, as the integrator's backend asserts it when minting
 * the reporter token (§5.4). Shown to triage members, never to
 * reporters.
 */
export interface ReporterProfileInput {
  /** An `https:` or `http:` image URL. */
  avatar_url?: string | null;
  email?: string | null;
  /** A GitHub login, for asking the reporter to review a fix. */
  github?: string | null;
  name?: string | null;
}

/** A reporter token (`cxr_…`), bound to a key, an origin and a reporter ID. */
export interface ReporterToken {
  expires_at: string;
  token: string;
}

/** A service grant's request for a reporter token. */
export interface ReporterTokenRequest {
  /** One of the queue's origins. */
  origin: string;
  /**
   * Who the backend says the reporter is; each report filed under the
   * token keeps it. Read as JSON first, so a bad one is 400
   * `invalid_profile` rather than `invalid_args`.
   */
  profile?: ReporterProfileInput | null;
  /** The reporter key's 32 public bytes in unpadded base64url. */
  public_key: string;
  reporter_id: string;
}

/** Whose reports a reporter token reads (§7.2). */
export type ReportersSee = "own" | "all";

/**
 * Why a report was closed (§6.1): `fixed` when a fix was verified or
 * triage closed it as fixed; the others when triage closed it without a
 * verdict.
 */
export type ResolveReason = "fixed" | "duplicate" | "wontfix" | "cannot_reproduce" | "other";

/** The page's scroll offsets in CSS pixels. */
export interface Scroll {
  x: number;
  y: number;
}

/**
 * What the server that received a report says about itself (§5.1), as
 * its `/healthz` reports it, and when it received the report.
 */
export interface ServerStamp {
  build?: string;
  commit?: string;
  received_at: string;
  version: string;
}

/**
 * A new service grant: its token (`cxq_…`) and its Ed25519 key, shown in
 * this answer alone.
 */
export interface ServiceGrantCreated {
  grant: ServiceGrantInfo;
  /** The key's PKCS#8 v2 document, unpadded base64url. */
  private_key: string;
  token: string;
}

/** A service grant as its queue's admins see it. */
export interface ServiceGrantInfo {
  created_at: string;
  id: string;
  name: string;
}

/** `POST /api/queues/<qid>/service-grants` body. */
export interface ServiceGrantRequest {
  /** What the grant is called: 1 to 120 characters. */
  name: string;
}

/**
 * A reporter's verdict that a fix works: a note, and its browser's
 * context at the verdict (checked and scrubbed as a filing's). An empty
 * body is `{}`.
 */
export interface VerifyRequest {
  /** Read as JSON first, so a bad one is 400 `invalid_context`. */
  context?: ReportContextInput | null;
  note?: string | null;
}

/** The viewport in CSS pixels, and its device pixel ratio. */
export interface Viewport {
  dpr: number;
  h: number;
  w: number;
}

/** Each public operation, by `operationId`: its method and path, its
 * bodies, the `error.code`s it answers, and the contract's `x-clax-*`
 * extensions (retry, channel, rights, stream). */
export interface Operations {
  /** An agent's session claims a report under a lease and owns its thread. */
  claim_report: {
    method: "POST";
    path: "/api/queues/{qid}/reports/{tid}/claim";
    query: never;
    request: never;
    status: 200;
    response: Lease;
    errors: "body_too_large" | "busy" | "forbidden_origin" | "invalid_request" | "invalid_signature" | "invalid_token" | "limit_reached" | "no_session" | "not_a_member" | "not_claimable" | "not_found" | "not_permitted" | "replayed_request" | "stale_request" | "thread_taken" | "timeout" | "unauthorized";
    retry: true;
    channel: "token" | "grant";
    rights: { action: "comments.reply" };
    stream: false;
  };
  /** An admin makes a service grant; its token and key are in this answer alone. */
  create_service_grant: {
    method: "POST";
    path: "/api/queues/{qid}/service-grants";
    query: never;
    request: ServiceGrantRequest;
    status: 201;
    response: ServiceGrantCreated;
    errors: "forbidden_origin" | "invalid_args" | "invalid_name" | "not_a_member" | "not_a_person" | "not_for_agents" | "not_permitted" | "not_queue_admin" | "unauthorized";
    retry: false;
    channel: "session" | "token";
    rights: { queue_role: "admin" };
    stream: false;
  };
  /** What this Clax is, for any caller. */
  discover: {
    method: "GET";
    path: "/.well-known/clax";
    query: never;
    request: never;
    status: 200;
    response: Discovery;
    errors: never;
    retry: true;
    channel: "anonymous";
    rights: null;
    stream: false;
  };
  /** The token's reporter files a report on an external artifact. */
  file_report: {
    method: "POST";
    path: "/api/queues/{qid}/reporter/reports";
    query: never;
    request: FormData;
    status: 201;
    response: ReportAnswer;
    errors: "body_too_large" | "busy" | "external_artifact" | "invalid_anchor" | "invalid_args" | "invalid_clip" | "invalid_context" | "invalid_diagnostics" | "invalid_external_id" | "invalid_metadata" | "invalid_multipart" | "invalid_request" | "invalid_signature" | "invalid_token" | "limit_reached" | "metadata_too_large" | "replayed_request" | "stale_request" | "timeout" | "token_expired" | "wrong_origin";
    retry: false;
    channel: "reporter";
    rights: null;
    stream: false;
  };
  /** The live queues the caller made or is a member of, with its role on each. */
  list_queues: {
    method: "GET";
    path: "/api/queues";
    query: never;
    request: never;
    status: 200;
    response: QueueList;
    errors: "body_too_large" | "busy" | "forbidden_origin" | "invalid_request" | "invalid_signature" | "invalid_token" | "limit_reached" | "not_permitted" | "replayed_request" | "stale_request" | "timeout" | "unauthorized";
    retry: true;
    channel: "session" | "token" | "grant";
    rights: null;
    stream: false;
  };
  /** A service grant mints a reporter token for one reporter, origin and key. */
  mint_reporter_token: {
    method: "POST";
    path: "/api/queues/{qid}/reporter-tokens";
    query: never;
    request: ReporterTokenRequest;
    status: 200;
    response: ReporterToken;
    errors: "body_too_large" | "busy" | "forbidden_origin" | "invalid_args" | "invalid_key" | "invalid_origin" | "invalid_profile" | "invalid_reporter_id" | "invalid_request" | "invalid_signature" | "invalid_token" | "limit_reached" | "replayed_request" | "stale_request" | "timeout" | "wrong_origin";
    retry: true;
    channel: "service";
    rights: null;
    stream: false;
  };
  /** A page of the reports the token's reporter sees, in order of arrival. */
  my_reports: {
    method: "GET";
    path: "/api/queues/{qid}/reporter/reports";
    query: { cursor?: string };
    request: never;
    status: 200;
    response: ReportPage;
    errors: "body_too_large" | "busy" | "invalid_query" | "invalid_request" | "invalid_signature" | "invalid_token" | "limit_reached" | "replayed_request" | "stale_request" | "timeout" | "token_expired" | "wrong_origin";
    retry: true;
    channel: "reporter";
    rights: null;
    stream: false;
  };
  /** The lease's session extends its claim. */
  renew_lease: {
    method: "POST";
    path: "/api/queues/{qid}/reports/{tid}/renew";
    query: never;
    request: never;
    status: 200;
    response: Lease;
    errors: "body_too_large" | "busy" | "claim_required" | "forbidden_origin" | "invalid_request" | "invalid_signature" | "invalid_token" | "lease_lost" | "limit_reached" | "no_session" | "not_a_member" | "not_found" | "not_permitted" | "replayed_request" | "stale_request" | "thread_taken" | "timeout" | "unauthorized";
    retry: true;
    channel: "token" | "grant";
    rights: { action: "comments.reply" };
    stream: false;
  };
  /** What waits for the token's reporter. */
  reporter_badge: {
    method: "GET";
    path: "/api/queues/{qid}/reporter/badge";
    query: never;
    request: never;
    status: 200;
    response: Badge;
    errors: "body_too_large" | "busy" | "invalid_request" | "invalid_signature" | "invalid_token" | "limit_reached" | "replayed_request" | "stale_request" | "timeout" | "token_expired" | "wrong_origin";
    retry: true;
    channel: "reporter";
    rights: null;
    stream: false;
  };
  /** The clip of a report the token's reporter sees. */
  reporter_clip: {
    method: "GET";
    path: "/api/queues/{qid}/reporter/reports/{tid}/clip";
    query: never;
    request: never;
    status: 200;
    response: Blob;
    errors: "body_too_large" | "busy" | "invalid_request" | "invalid_signature" | "invalid_token" | "limit_reached" | "not_found" | "replayed_request" | "stale_request" | "timeout" | "token_expired" | "wrong_origin";
    retry: true;
    channel: "reporter";
    rights: null;
    stream: false;
  };
  /** The token's reporter comments on a report it sees. */
  reporter_comment: {
    method: "POST";
    path: "/api/queues/{qid}/reporter/reports/{tid}/comments";
    query: never;
    request: CommentRequest;
    status: 200;
    response: ReportAnswer;
    errors: "body_too_large" | "busy" | "invalid_args" | "invalid_comment" | "invalid_request" | "invalid_signature" | "invalid_token" | "limit_reached" | "not_found" | "replayed_request" | "stale_request" | "timeout" | "token_expired" | "wrong_origin";
    retry: false;
    channel: "reporter";
    rights: null;
    stream: false;
  };
  /** The clip a reopen's comment carries, on a report the token's reporter sees. */
  reporter_comment_clip: {
    method: "GET";
    path: "/api/queues/{qid}/reporter/reports/{tid}/comments/{cid}/clip";
    query: never;
    request: never;
    status: 200;
    response: Blob;
    errors: "body_too_large" | "busy" | "invalid_request" | "invalid_signature" | "invalid_token" | "limit_reached" | "not_found" | "replayed_request" | "stale_request" | "timeout" | "token_expired" | "wrong_origin";
    retry: true;
    channel: "reporter";
    rights: null;
    stream: false;
  };
  /** The diagnostics of one capture of a report the token's reporter filed. */
  reporter_diagnostics: {
    method: "GET";
    path: "/api/queues/{qid}/reporter/reports/{tid}/diagnostics";
    query: { capture?: number };
    request: never;
    status: 200;
    response: DiagnosticsAnswer;
    errors: "body_too_large" | "busy" | "invalid_query" | "invalid_request" | "invalid_signature" | "invalid_token" | "limit_reached" | "not_found" | "replayed_request" | "stale_request" | "timeout" | "token_expired" | "wrong_origin";
    retry: true;
    channel: "reporter";
    rights: null;
    stream: false;
  };
  /** The token's reporter says a report it filed is still broken: a new capture. */
  reporter_reopen: {
    method: "POST";
    path: "/api/queues/{qid}/reporter/reports/{tid}/reopen";
    query: never;
    request: FormData;
    status: 200;
    response: ReportAnswer;
    errors: "body_too_large" | "busy" | "invalid_args" | "invalid_clip" | "invalid_comment" | "invalid_context" | "invalid_diagnostics" | "invalid_multipart" | "invalid_request" | "invalid_signature" | "invalid_token" | "limit_reached" | "not_fix_proposed" | "not_found" | "replayed_request" | "stale_request" | "timeout" | "token_expired" | "wrong_origin";
    retry: false;
    channel: "reporter";
    rights: null;
    stream: false;
  };
  /** The token's reporter has read a report it sees. */
  reporter_seen: {
    method: "POST";
    path: "/api/queues/{qid}/reporter/reports/{tid}/seen";
    query: never;
    request: never;
    status: 204;
    response: never;
    errors: "body_too_large" | "busy" | "invalid_request" | "invalid_signature" | "invalid_token" | "limit_reached" | "not_found" | "replayed_request" | "stale_request" | "timeout" | "token_expired" | "wrong_origin";
    retry: true;
    channel: "reporter";
    rights: null;
    stream: false;
  };
  /** The token's reporter says the fix waiting on a report it filed works. */
  reporter_verify: {
    method: "POST";
    path: "/api/queues/{qid}/reporter/reports/{tid}/verify";
    query: never;
    request: VerifyRequest;
    status: 200;
    response: ReportAnswer;
    errors: "body_too_large" | "busy" | "invalid_args" | "invalid_comment" | "invalid_context" | "invalid_request" | "invalid_signature" | "invalid_token" | "limit_reached" | "not_fix_proposed" | "not_found" | "replayed_request" | "stale_request" | "timeout" | "token_expired" | "wrong_origin";
    retry: false;
    channel: "reporter";
    rights: null;
    stream: false;
  };
}

/** Every `error.code` an operation of the contract answers. */
export type ErrorCode = Operations[keyof Operations]["errors"];
