// The report-queue routes' JSON, written by hand until the contract is
// generated from the routes (spec 2026-10-27 §4.1). Field names are the
// wire's.

/** A queue's `reporters_see`: what a reporter token may read. */
export type ReportersSee = "own" | "all";
/** A member's role on a queue. */
export type QueueRole = "admin" | "triage";
/** A report's thread status. */
export type ReportStatus = "open" | "resolved";
/** A report's place in triage (spec 2026-10-29-feedback-loop-design §6.1):
 * `open`, waiting for someone; `claimed`, while a live lease holds it;
 * `fix_proposed`, while a fix waits for the reporter's verdict; `verified`,
 * once the reporter said it is fixed; `resolved`, once triage closed it
 * without a verdict. `verified` and `resolved` resolve its thread. */
export type ReportState = "open" | "claimed" | "fix_proposed" | "verified" | "resolved";
/** Why a `verified` or `resolved` report was closed. */
export type ResolveReason = "fixed" | "duplicate" | "wontfix" | "cannot_reproduce" | "other";
/** What became of a fix attempt: waiting for a verdict, verified, rejected
 * (the reporter reopened), superseded by a later fix, or closed (the report
 * was resolved before a verdict). */
export type FixOutcome = "pending" | "verified" | "rejected" | "superseded" | "closed";
/** Who proposed a fix: an agent through its session, or a person. */
export type ProposerKind = "agent" | "person";
/** Who wrote a comment on a report. */
export type AuthorKind = "viewer" | "agent" | "reporter";

/** `Queue`: what `GET /api/queues` lists, with the caller's role. */
export interface Queue {
  id: string;
  name: string;
  origins: string[];
  reporters_see: ReportersSee;
  created_at: string;
  role: QueueRole;
}

/** The external artifact a report is filed on. */
export interface ReportArtifact {
  id: string;
  external_id: string;
  title: string;
  url: string;
  metadata: unknown;
  metadata_version: number;
  revision: string | null;
}

/** One comment on a report. A reopen's comment carries its capture: the
 * reopen's number (the filing being capture 0), whether a clip came with it
 * (`…/comments/<cid>/clip`) and how many diagnostics lines
 * (`…/diagnostics?capture=<n>`); each is absent on any other comment.
 * `triage_only` marks a note for the queue's members alone (a person's
 * release of a claim): a reporter's view leaves it out. */
export interface ReportComment {
  id: string;
  author_kind: AuthorKind;
  author_name: string;
  body: string;
  created_at: string;
  capture?: number;
  has_clip?: boolean;
  diagnostics_lines?: number;
  triage_only?: boolean;
}

/** A live claim on a report. */
export interface ReportClaim {
  holder_name: string;
  expires_at: string;
}

/** `Report`: a thread filed into a queue, on an external artifact. */
export interface Report {
  id: string;
  queue_id: string;
  artifact: ReportArtifact;
  anchor: unknown;
  status: ReportStatus;
  state: ReportState;
  reporter_id: string | null;
  /** On a report served to a reporter: whether that reporter filed it.
   * Only they give it a verdict or reopen it. Absent on a member's read. */
  mine?: boolean;
  /** The host app's metadata; `null` on a report served to a reporter
   * that did not file it. */
  metadata: unknown;
  metadata_version: number;
  has_clip: boolean;
  comments: ReportComment[];
  claim: ReportClaim | null;
  created_at: string;
  /** What the reporter's browser said at capture; absent when none was
   * sent, and on a report served to a reporter that did not file it. */
  context?: ReportContext;
  /** The receiving server's stamp; absent on a report filed without one. */
  server?: ServerStamp;
  /** The reporter's profile, served to triage members only. */
  reporter_profile?: ReporterProfile;
  /** Why a `verified` or `resolved` report was closed. */
  resolve_reason?: ResolveReason;
  /** The fixes proposed on it, oldest first; absent when none was. */
  fixes?: FixAttempt[];
  /** How many times it was reopened; absent when never. */
  reopen_count?: number;
  /** How many diagnostics lines its latest capture holding any has; absent when none has. */
  diagnostics_lines?: number;
  /** How many diagnostics lines the filing itself (capture 0) has; absent when none. */
  filing_diagnostics_lines?: number;
}

/** What the reporter's browser said at capture (spec
 * 2026-10-29-feedback-loop-design §5.1): at most 16 KiB as compact JSON,
 * with no top-level key but these (else 400 `invalid_context`). Clax
 * scrubs `environment.url`, `environment.route`, `environment.title` and
 * every string of `app` again on receipt. Served to triage and to the
 * reporter that filed the report; another reporter (`reporters_see: all`)
 * is served the report without it, and without any verdict's context. */
export interface ReportContext {
  /** The shape's version: 1. */
  v: 1;
  /** When the capture was made, RFC 3339. */
  captured_at: string;
  release?: Release;
  /** The host app's feature flags at capture. */
  flags?: Record<string, FlagValue>;
  environment?: Environment;
  capture?: CaptureInfo;
  /** Opaque JSON the host app adds. */
  app?: unknown;
}

/** The release of the host app the reporter ran, as the host names it. */
export interface Release {
  id?: string;
  version?: string;
  commit?: string;
  channel?: string;
}

/** A feature flag's value. The SDK leaves out a flag whose number is not
 * finite (`NaN`, `Infinity`), which JSON cannot carry. */
export type FlagValue = boolean | number | string;

/** The page and browser a capture was made in. `url` keeps its query keys,
 * never their values. */
export interface Environment {
  url?: string;
  route?: string;
  title?: string;
  /** CSS pixels, and the device pixel ratio. */
  viewport?: { w: number; h: number; dpr: number };
  /** The page's scroll offsets in CSS pixels. */
  scroll?: { x: number; y: number };
  user_agent?: string;
  platform?: string;
  locale?: string;
  time_zone?: string;
  /** As the page's `prefers-color-scheme` matched. */
  color_scheme?: "light" | "dark";
}

/** What the reporter captured. */
export type CaptureMode = "element" | "range" | "area" | "screen";
/** How the clip was made: a render of the page's DOM, or the browser's screen capture. */
export type CaptureMethod = "dom" | "display";

/** How a report's clip was captured. */
export interface CaptureInfo {
  mode: CaptureMode;
  method: CaptureMethod;
  /** What a DOM render left out (`iframe`, `image`, `canvas`, `video`, …),
   * and where: `rect` is in the clip image's own pixels, from its top left
   * corner, so it outlines the omission exactly on the image. */
  omitted?: { kind: string; rect: { x: number; y: number; w: number; h: number } }[];
}

/** What the server that received a report says about itself, as its
 * `/healthz` reports it, and when it received the report. */
export interface ServerStamp {
  version: string;
  commit?: string;
  build?: string;
  received_at: string;
}

/** Who a reporter is, as the integrator's backend asserts it when minting
 * its token: each field at most 256 bytes, `avatar_url` an `https:` or
 * `http:` URL (else 400 `invalid_profile`). */
export interface ReporterProfile {
  name?: string;
  email?: string;
  /** A GitHub login, for asking the reporter to review a fix. */
  github?: string;
  avatar_url?: string;
}

/** A fix proposed on a report, numbered from 1 in the order proposed,
 * with its verdict once given. */
export interface FixAttempt {
  n: number;
  /** Plain text, posted to the report as a comment the reporter sees. */
  summary: string;
  /** The change: a pull request, a commit page, a ticket. */
  url?: string;
  commit?: string;
  /** The release the fix ships in. */
  release?: string;
  /** Where the reporter checks the fix: a preview deployment's origin
   * once its fix ticket attached it (`ServiceClient.attachPreview`). */
  verify_url?: string;
  /** When the reporter can check it, such as `release:0.6.1`. */
  verify_after?: string;
  proposer_name: string;
  /** `agent` when an agent's session proposed it: while it waits, that
   * session hears the reporter's replies and may answer them. `person`
   * when a triage member did: replies wait for the queue's people.
   * Absent from a server older than the field: read it as `agent`. */
  proposer_kind?: ProposerKind;
  created_at: string;
  outcome: FixOutcome;
  verdict_note?: string;
  /** The verdict-giver's browser context at the verdict; absent on a
   * report served to a reporter that did not file it. */
  verdict_context?: ReportContext;
  verdict_at?: string;
  /** The exact origin the reporter verified it from (its reporter
   * token's): a preview deployment's, or the production site's. Absent
   * for a verdict a triage person gave, and from a server older than the
   * field. */
  verified_origin?: string;
}

/** A line of a report's diagnostics (spec 2026-10-29-feedback-loop-design
 * §5.2): `kind` `error` is an uncaught error, `rejection` an unhandled
 * promise rejection, `request` a failed request. Clax scrubs `text` again
 * on receipt. */
export interface DiagLine {
  /** When it happened, RFC 3339, at most 64 bytes. */
  at: string;
  level: "debug" | "info" | "warn" | "error";
  kind: "console" | "error" | "rejection" | "request";
  text: string;
}

/** `GET /api/queues/<qid>/reporter/badge`: the reports waiting on the
 * reporter (a fix waiting for its verdict, or a comment not its own since
 * it last read them), oldest first; `waiting` is `report_ids.length`. */
export interface Badge {
  waiting: number;
  report_ids: string[];
}

/** `Lease`: what a claim or a renewal answers. */
export interface Lease {
  lease_id: string;
  expires_at: string;
  fence: number;
  report: Report;
}

/** A page of reports; `next` is the cursor of the following page. */
export interface ReportPage {
  reports: Report[];
  next: string | null;
}

/** `POST /api/queues/{qid}/reporter-tokens`'s body. */
export interface ReporterTokenRequest {
  reporter_id: string;
  origin: string;
  /** The reporter key's 32 public bytes in unpadded base64url. */
  public_key: string;
  /** Who the backend says the reporter is; each report filed under the token keeps it. */
  profile?: ReporterProfile;
}

/** A reporter token: `cxr_…`, bound to a key, an origin and a reporter ID. */
export interface ReporterToken {
  token: string;
  expires_at: string;
}

/** `POST /api/queues/{qid}/service-grants`'s answer: the token and key are shown once. */
export interface ServiceGrantCreated {
  /** `origins`: the queue's origins and patterns the grant mints for
   * (such as its preview pattern alone); `null` for every one. */
  grant: { id: string; name: string; origins: string[] | null; created_at: string };
  token: string;
  /** PKCS#8 of the grant's Ed25519 key, unpadded base64url. */
  private_key: string;
}

/** A fix's ticket: `POST /api/queues/{qid}/reports/{tid}/fix` with
 * `preview: true` answers it beside the report, once. Its holder, the
 * fix's preview deployment, can only attach its own origin to that fix
 * (`ServiceClient.attachPreview`); it ends at `expires_at` (14 days) or at
 * the fix's verdict, whichever comes first. */
export interface FixTicket {
  /** `cxf_<queue ID>.<report ID>.<fix number>.<secret>`: what the
   * preview deployment holds, as `CLAX_FIX_TICKET`. */
  token: string;
  expires_at: string;
  /** The fix's number. */
  n: number;
  /** The queue's origin patterns; the preview's origin must match one. */
  origins: string[];
  /** The route the ticket is presented on. */
  attach_path: string;
}

/** What `ServiceClient.attachPreview` sends. */
export interface PreviewAttachment {
  /** The fix ticket (`CLAX_FIX_TICKET`). */
  ticket: string;
  /** The preview deployment's own exact origin, from its deployment's
   * configuration (never from a request it received), such as
   * `https://app-pr-538.fly.dev`. */
  origin: string;
  /** Where on it to check the fix, starting with `/`. Default `/`. */
  path?: string;
}

/** `POST /api/queues/{qid}/reports/{tid}/fixes/{n}/preview`'s answer:
 * the report (as triage reads it), the fix's `verify_url` now, and
 * whether this call changed it (`false` when it named that URL already). */
export interface PreviewAttached {
  report: Report;
  verify_url: string;
  changed: boolean;
}

/** A webhook event's type (`clax_core::webhook::Event`). */
export type WebhookEventType =
  | "report.created"
  | "report.claimed"
  | "report.released"
  | "report.replied"
  | "report.fix_proposed"
  | "report.verified"
  | "report.reopened"
  | "report.resolved";

/** A webhook endpoint's state: `unverified` until it answers a
 * verification (sent no report event meanwhile), `verified`, `disabled`
 * after deliveries given up in a row, `expired` when never verified within
 * a day of being added (inert until removed). */
export type WebhookState = "unverified" | "verified" | "disabled" | "expired";

/** A queue's webhook endpoint as its admins read it: never its secret. */
export interface Webhook {
  id: string;
  url: string;
  events: WebhookEventType[];
  created_at: string;
  state: WebhookState;
  verified_at: string | null;
  /** Until when an unverified endpoint may be verified; `null` once it is. */
  verify_by: string | null;
  disabled_at: string | null;
  /** Deliveries given up in a row since the last one delivered. */
  given_up: number;
  /** Since when its deliveries fail: set when one is given up, cleared when one is delivered. */
  failing_since: string | null;
  /** Why its latest given-up delivery, or its latest verification, failed. */
  last_error: string | null;
  /** Events dropped for it past the hourly cap, ever. */
  dropped: number;
  last_dropped_at: string | null;
}

/** `POST /api/queues/{qid}/webhooks`'s answer: the secret is shown once. */
export interface WebhookCreated {
  webhook: Webhook;
  /** `whsec_…`: what `verifyWebhook` checks each delivery with. */
  secret: string;
}

/** One delivery of an event to an endpoint, as its log shows it. */
export interface WebhookDelivery {
  id: string;
  webhook_id: string;
  event_id: string;
  event: WebhookEventType | "webhook.verify";
  /** `null` for a verification. */
  report_id: string | null;
  state: "pending" | "delivered" | "failed";
  attempts: number;
  next_at: string | null;
  last_at: string | null;
  last_status: number | null;
  last_error: string | null;
  delivered_at: string | null;
  created_at: string;
}

/** The report a webhook event is about, as the queue's triage members read it. */
export interface WebhookReport {
  /** The report's ID: what an app's `?clax-report=<id>` names. */
  id: string;
  state: ReportState;
  external_id: string;
  artifact_id: string;
  title: string;
  /** The page it was filed on. */
  page_url: string;
  /** The report in its queue on the Clax instance, for the queue's members. */
  link?: string;
  reporter_id: string | null;
  /** The profile the reporter's token was minted with: personal data. */
  reporter: ReporterProfile | null;
  resolve_reason?: ResolveReason;
  reopen_count: number;
  created_at: string;
}

/** The latest fix, on `report.fix_proposed`, `report.verified` and `report.reopened`. */
export interface WebhookFix {
  n: number;
  summary: string;
  url?: string;
  commit?: string;
  release?: string;
  verify_url?: string;
  verify_after?: string;
  outcome: FixOutcome;
  proposer_name: string;
}

/** The reply on `report.replied`; the reopen's comment on `report.reopened`. */
export interface WebhookComment {
  id: string;
  author_kind: AuthorKind;
  author_name: string;
  body: string;
  created_at: string;
}

/** What one delivery of a report event posts. `id` is the same on every
 * attempt and redelivery: drop repeats by it. */
export interface ReportWebhookEvent {
  id: string;
  type: WebhookEventType;
  created_at: string;
  queue: { id: string; name: string };
  report: WebhookReport;
  fix?: WebhookFix;
  comment?: WebhookComment;
}

/** What an endpoint answers a verification with: 2xx and this JSON. */
export interface WebhookVerifyReply {
  /** The hex HMAC-SHA256, under the endpoint's secret, of `webhook.verify.<challenge>`. */
  answer: string;
}

/** The verification Clax sends an endpoint when it is added (and on
 * Verify again): it is sent no report event until it answers 2xx with
 * `reply` as its JSON body. */
export interface WebhookVerifyEvent {
  id: string;
  type: "webhook.verify";
  created_at: string;
  queue: { id: string; name: string };
  webhook: { id: string; url: string };
  challenge: string;
  /** Set by `verifyWebhook`, not sent: the body to answer with. */
  reply: WebhookVerifyReply;
}

/** What one webhook delivery posts: a report event, or the endpoint's verification. */
export type WebhookEvent = ReportWebhookEvent | WebhookVerifyEvent;

/** `GET /.well-known/clax`. */
export interface Discovery {
  kind: "local" | "hosted" | "worker";
  contract_version: string;
  sign_in: string[];
  capabilities: string[];
}

/** Every error answer's body (`ApiError`). */
export interface ApiErrorBody {
  error: { code: string; message: string };
}

/** Error codes the queue routes answer (`ApiError`'s `error.code`): the
 * reporter routes, the service route that mints reporter tokens, the fix
 * ticket's route, the signed-request checks in front of them, and the
 * triage routes.
 * `web/sdk/test/queue-error-codes.json` lists the same codes, and
 * Clax's own tests of the reporter routes fail on a code it lacks. */
export type QueueErrorCode =
  // The credential and the signed request, on every queue route.
  | "invalid_token"
  | "token_expired"
  | "invalid_signature"
  | "stale_request"
  | "replayed_request"
  | "forbidden_origin"
  | "wrong_origin"
  | "limit_reached"
  | "body_too_large"
  | "busy"
  | "timeout"
  // Minting a reporter token.
  | "invalid_origin"
  | "invalid_reporter_id"
  | "invalid_key"
  | "invalid_profile"
  // Filing, reopening, commenting and reading.
  | "invalid_args"
  | "invalid_multipart"
  | "invalid_anchor"
  | "invalid_metadata"
  | "metadata_too_large"
  | "invalid_clip"
  | "invalid_comment"
  | "invalid_context"
  | "invalid_diagnostics"
  | "invalid_query"
  | "external_artifact"
  | "not_found"
  | "not_fix_proposed"
  // Triage.
  | "not_a_member"
  | "not_for_agents"
  | "not_claimable"
  | "claim_required"
  | "lease_lost"
  | "thread_taken"
  | "invalid_state"
  | "invalid_fix"
  | "no_preview_origins"
  | "no_session";

/** Error codes the SDK gives itself: `network` (status 0) when no answer
 * came, `bad_response` for an answer that is not the JSON expected, and
 * `unsupported_browser` (status 0) where WebCrypto lacks Ed25519. */
export type SdkErrorCode = "network" | "bad_response" | "unsupported_browser";

/** `ClaxError.code`: a queue route's code, the SDK's own, or another an
 * instance answers (`http_<status>` for an answer without the error shape). */
export type ClaxErrorCode = QueueErrorCode | SdkErrorCode | (string & {});
