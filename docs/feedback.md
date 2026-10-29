# Bug reports from your app to your agents

This guide is for a product that wants its users to report problems from
inside the app and have agents fix them. The user points at what is
wrong, Clax files the report into a **report queue** with a screenshot and
what the browser knew, an agent claims it and proposes a fix, and the user
says whether the fix works. Nothing here is specific to a framework: any
web app with a backend of its own can do it, whatever it is built with.

clax.dev does all of this for its own people, using only what this guide
describes. It is the worked example below
(`crates/clax-server/src/integrations/feedback.rs`).

The exact routes, fields, limits and error codes are in
[`docs/contract.md`](contract.md), "Report queue routes" and "Report queue
tools". This guide says how the parts fit together.

## The moving parts

```
 your page (browser)                your backend                 Clax instance
 ───────────────────                ────────────                 ─────────────
 <script src=".../_clax/overlay.js">
 Clax.overlay({instance, queue,
   auth, context, ...})
   │  auth(publicKey) ──────────────▶ POST /your/clax-token
   │                                  (your sign-in decides
   │                                   who may report)
   │                                  ServiceClient.mintReporterToken
   │                                    ─────────────────────────────▶ POST /api/queues/<qid>/reporter-tokens
   │                                                                   (signed with the service grant's key)
   │  ◀──────────────────────────────── {token, expires_at} ◀──────────
   │
   │  file / verify / reopen, signed with the browser's own key ──────▶ /api/queues/<qid>/reporter/...
   │                                                                     │
   │                                                                     ▼
   │                                       triage people: /queues/<qid> in Clax
   │                                       agents: queue_* MCP tools, clax queue ...
```

- **The queue** lives on a Clax instance (clax.dev, or one you host). Its
  admin creates it on the Queues page (`/queues`), names the origins your
  pages are served from, and chooses whether a reporter sees only their
  own reports or every report in the queue.
- **The service grant** is your backend's credential for the queue: a
  `cxq_` token and an Ed25519 private key, shown once when the admin makes
  it (Queues page, Manage, "Create service grant"). Keep it in your
  backend's secrets. It never goes to a browser.
- **The reporter token** (`cxr_…`) is what one user's browser files with.
  Your backend mints it for a user it has signed in, bound to a key the
  browser made and to your page's origin. The browser signs every request
  with that key, so a copied token is of no use on its own.
- **The overlay** is one script the instance serves to any origin. It
  draws the Report button, the capture modes, the composer and "Your
  reports", records recent errors, and talks to the reporter routes.
- **Triage** is the queue's members: people in the Clax UI, and their
  agents through the MCP tools or the CLI.

## 1. Make the queue and its service grant

On the instance, as the person who will administer the queue:

1. Queues, New queue. Give it a name, list your origins one per line
   (`https://app.example.com`, and `http://localhost:5173` for
   development if you want reports from there), and pick who reporters see
   ("Their own reports" is the usual choice).
2. Manage, add the people who triage (Members), then "Create service
   grant". Copy the token and the private key into your backend's secrets,
   for example `CLAX_QUEUE`, `CLAX_QUEUE_TOKEN` and `CLAX_QUEUE_KEY`.
   They are shown once; make a new grant to rotate, then delete the old
   one.

The same steps as API calls are in `docs/prod.md`, "Feedback: clax.dev's
own report queue".

## 2. The token route in your backend

Your backend adds one route that only signed-in users may call. It takes
the browser's public key and answers a reporter token for that user. This
is where you decide who may report: check your own session, plan, role or
feature flag, and refuse everyone else.

The `reporter_id` you give is how Clax tells your users apart (it decides
which reports a user sees in "Your reports"), so use a stable ID from your
own user table, never an email address. The optional `profile` (`name`,
`email`, `github`, `avatar_url`, each at most 256 bytes) is shown to
triage members with each report, and never to other reporters. A
`github` login lets a fixing agent ask the reporter to review its pull
request.

### TypeScript (Node 20 or later, Workers, Deno, Bun)

`ServiceClient` (`web/sdk/src/service.ts`) signs the request with
WebCrypto. It is a small file with no dependencies; copy `service.ts`,
`client.ts`, `sign.ts` and `types.ts` from `web/sdk/src` into your
backend.

```ts
import { ServiceClient } from "./clax/service";

const clax = new ServiceClient({
  instance: "https://clax.dev",          // the instance's base URL
  queue: process.env.CLAX_QUEUE!,        // the queue's ID
  token: process.env.CLAX_QUEUE_TOKEN!,  // the service grant's cxq_ token
  privateKey: process.env.CLAX_QUEUE_KEY!, // its private_key, as shown
});

// POST /clax-token {public_key}, behind your own sign-in.
app.post("/clax-token", async (req, res) => {
  const user = req.session.user;
  if (!user) return res.status(401).json({ error: "sign in first" });
  if (!user.flags.reportsProblems) return res.status(403).json({ error: "not for you" });
  const { public_key } = req.body;
  if (typeof public_key !== "string" || !public_key) return res.status(400).json({ error: "public_key" });
  const minted = await clax.mintReporterToken({
    reporter_id: user.id,
    origin: "https://app.example.com",   // the page's origin: one of the queue's
    public_key,
  }, { name: user.name, github: user.githubLogin });
  res.set("cache-control", "no-store").json(minted); // {token, expires_at}
});
```

The end-to-end test's deployer site (`web/e2e/deployer-site/server.ts`) is
a complete backend of this shape in under 200 lines.

### Rust

The request is `POST /api/queues/<qid>/reporter-tokens` with
`Authorization: Clax-Queue <token>` and a `Clax-Signature` by the service
grant's key over the canonical lines that `docs/contract.md` ("Signed
requests") lists. A Rust backend can depend on `clax-core` for
`clax_core::sig` (the signer the instance verifies with) or implement the
lines with any Ed25519 crate.

clax.dev's own token route is written this way. Its backend half uses
only what an outside backend has (a queue ID, the grant's token and key,
and the public route) and is quoted here from
`crates/clax-server/src/integrations/feedback.rs`:

```rust
/// The request an integrator's backend sends to mint a reporter token for
/// `reporter_id`, bound to the browser's `public_key`, signed at `now`
/// (Unix seconds).
pub fn mint_request(
    i: &FeedbackIntegration,
    now: i64,
    reporter_id: &str,
    public_key: &str,
    profile: Option<&Profile>,
) -> Result<Request<Body>, String> {
    let key = KeyPair::from_pkcs8(&i.grant_key_pkcs8)
        .map_err(|_| "the service grant's key is not an Ed25519 PKCS#8 key".to_string())?;
    let path = format!("/api/queues/{}/reporter-tokens", i.queue_id);
    let host = host_of(&i.origin)?;
    let body = serde_json::to_vec(&MintBody {
        reporter_id,
        origin: &i.origin,
        public_key,
        profile,
    })
    .expect("a mint body serialises");
    let nonce = sig::new_nonce();
    let signature = key
        .sign(&SignInput {
            method: "POST",
            path_and_query: &path,
            host: &host,
            t: now,
            nonce: &nonce,
            body_sha256: sha2::Sha256::digest(&body).into(),
            token_sha256: Some(clax_core::secret::sha256(&i.grant_token)),
            agent_headers_sha256: sig::no_agent_headers_sha256(),
        })
        .map_err(|e| format!("signing the request: {e}"))?
        .to_string();
    Request::builder()
        .method("POST")
        .uri(&path)
        .header(header::HOST, host)
        .header("authorization", format!("Clax-Queue {}", i.grant_token))
        .header("clax-signature", signature)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONTENT_LENGTH, body.len())
        .body(Body::from(body))
        .map_err(|e| e.to_string())
}
```

`integration(queue_id, origin, grant_json)` in the same file reads the
grant (`{token, private_key_pkcs8}`, the key as the Queues page shows it)
and checks it once at start. clax.dev sends the request to its own router
in process (`mint`); an outside backend sends the same request over HTTP
to `https://<instance><path>` with any client (`reqwest`, `hyper`).

clax.dev's route around it, `token`, does what yours will: it refuses
agents and anyone without the `feedback` flag (403 `no_flag`), uses the
person's public ID as `reporter_id` and their GitHub name and login as the
profile, passes a 400 about the browser's key back as a 400, and turns any
other refusal into 503 `feedback_unavailable` (logged), since that one is
the integration's own fault (a revoked grant, an origin the queue no
longer lists).

### What the token route sees

- **Every visitor who loads the overlay asks for a token.** "Your
  reports" polls the reporter's badge when the page loads, every minute
  while it is visible, and whenever it regains focus, and each poll needs
  a token. So the route is called for people who never file anything.
  Load the overlay only for users your own "may report" check accepts,
  rather than loading it for everyone and refusing them in the token
  route, or pass `canFile: false` to users who may read their reports but
  not file new ones.
- **Every reporter shares the service grant's limit.** Each mint is
  signed by the one grant key, and a key may sign at most 65,536 requests
  in any 240 seconds (429 `limit_reached`, with `Retry-After`). That is
  far above any product's sign-in rate, but a loop that mints on every
  request will reach it for everyone at once. Tokens last hours; the
  overlay keeps its token and asks again only when it expires.

## 3. Load the overlay on your pages

The instance serves the overlay to any origin, without credentials:

- `<instance>/_clax/overlay.js`: a classic script that defines
  `window.Clax`;
- `<instance>/_clax/overlay.mjs`: an ES module exporting the same names;
- `<instance>/_clax/overlay.d.ts`: the TypeScript types of both.

### A script tag

```html
<script src="https://clax.dev/_clax/overlay.js"></script>
<script src="/app.js"></script>
```

```js
// app.js
const clax = Clax.overlay({
  instance: "https://clax.dev",
  queue: "<queue ID>",
  auth: async publicKey => {
    const r = await fetch("/clax-token", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ public_key: publicKey }),
    });
    if (!r.ok) throw new Error(`no reporter token: ${r.status}`);
    return r.json(); // {token, expires_at}
  },
});
```

### A dynamic import, with types

```ts
// Save <instance>/_clax/overlay.d.ts as src/clax-overlay.d.ts and declare
// the module once:
//   declare module "https://clax.dev/_clax/overlay.mjs" { export * from "./clax-overlay"; }
const { overlay } = await import(/* @vite-ignore */ "https://clax.dev/_clax/overlay.mjs");
const clax = overlay({ instance, queue, auth });
```

The overlay touches `document` when it starts, so start it in the browser
only, once per page load, and close it when the page goes away.

### SvelteKit

```svelte
<!-- src/routes/+layout.svelte -->
<script lang="ts">
  import { onMount } from "svelte";
  import type { OverlayHandle } from "$lib/clax-overlay"; // overlay.d.ts, saved
  let { data, children } = $props(); // data.mayReport, data.release from +layout.server.ts

  onMount(() => {
    if (!data.mayReport) return; // your own check: no overlay, no token requests
    let clax: OverlayHandle | undefined;
    let closed = false;
    import(/* @vite-ignore */ "https://clax.dev/_clax/overlay.mjs").then(({ overlay }) => {
      if (closed) return;
      clax = overlay({
        instance: "https://clax.dev",
        queue: data.queue,
        auth: async (public_key: string) => {
          const r = await fetch("/clax-token", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ public_key }) });
          if (!r.ok) throw new Error(`no reporter token: ${r.status}`);
          return r.json();
        },
        context: () => ({ release: data.release, flags: data.flags }),
        nonce: data.cspNonce, // only with a strict style-src, below
      });
    });
    return () => { closed = true; clax?.close(); };
  });
</script>

{@render children()}
```

With a Rust backend serving the SvelteKit build, the token route above is
the backend's; nothing changes on the page.

### Content-Security-Policy

Add the instance to `script-src` and `connect-src`, and let the overlay's
preview images through:

```
script-src 'self' https://clax.dev;
connect-src 'self' https://clax.dev;
img-src 'self' data: blob:;
```

The overlay needs no inline script and no inline style: it styles itself
through constructed stylesheets, which `style-src` does not govern. One
thing does need your policy's help. Rendering the page into a screenshot
reads its web fonts through an inline `<style>`, which a strict
`style-src` (no `'unsafe-inline'`) refuses. Tell the overlay so, and it
draws screenshots in fallback fonts from the first one and trips no
violation:

- pass the nonce your policy gives inline styles, `Clax.overlay({...,
  nonce})` (any non-empty string, such as `"strict"`, will do where your
  policy uses hashes instead); or
- declare the policy in a `<meta http-equiv="Content-Security-Policy">`,
  which the overlay reads itself.

With the policy in a header only and no `nonce`, the first screenshot
trips one `style-src-elem` violation and that page's screenshots are
drawn in fallback fonts.

## 4. The overlay's options

`Clax.overlay(options)` returns a handle. Every option is in
`overlay.d.ts`:

| Option | |
|---|---|
| `instance` | The instance's base URL. |
| `queue` | The queue's ID. |
| `auth(publicKey)` | Gets a reporter token from your backend (section 2). |
| `launcher` | `"corner"` (default): Report and "Your reports" in the bottom right corner. `false`: none; you call `report()` and `showReports()` from your own menu. |
| `canFile` | Default `true`. `false` hides Report and keeps "Your reports". |
| `context` | Your `release`, `flags` and `app`, or a function giving them, called when each capture starts (section 5). |
| `metadata` | Any JSON attached to each report, or a function giving it, called once per report. Triage can edit it later. |
| `diagnostics` | `{default: true}` ticks "Include recent errors" by default (default unticked). |
| `externalId` | The record the report is about, in your terms (`invoice:4821`). Default: `<meta name="clax-artifact" content="…">`, else the page's URL without its route. |
| `nonce` | Your policy's style nonce (section 3). |

The handle:

- `report({mode?})` opens the capture bar (`"element"` by default, or
  `"area"`, `"screen"`);
- `showReports()` opens "Your reports";
- `on("badge", n => …)` hears how many reports wait for the user (a fix
  to check, or a reply), at once and on every change; `on("filed", r =>
  …)` hears each filed report; each returns its unsubscribe function;
- `close()` removes the overlay.

`Clax.diagnostics.install()` starts recording errors before the overlay
starts, for a page that loads it late. `Clax.CONTEXT_TIMEOUT_MS` (2000) is
how long your `context` and `metadata` functions may take; a function that
throws or takes longer files the report anyway, with
`app: {context_error}` in place of what it would have given.

clax.dev runs it with `launcher: false`: "Report a problem" and "My
reports" sit in the account menu, the badge beside the latter.

## 5. Context and flags

Each report carries a **context**, what the reporter's browser said at
the moment of capture. The overlay fills in the page and the browser; you
add your release and flags:

```js
Clax.overlay({
  ...,
  context: () => ({
    release: { version: "2.3.0", commit: BUILD_COMMIT, channel: "staging", id: BUILD_ID },
    flags: { new_editor: true, checkout: "variant-b", max_items: 50 },
    app: { repo: "acme/app", view: router.current.name },
  }),
});
```

- `release` fields are strings; `flags` values are booleans, numbers or
  strings; `app` is yours, small JSON. Clax reads one key of `app`:
  `repo` (`owner/name` on GitHub, or a repository URL) is where the triage
  view links `release.commit`.
- The overlay adds `environment` (URL, route, title, viewport, scroll,
  user agent, platform, locale, time zone, colour scheme) and `capture`
  (mode, and whether the screenshot is a render of the page or from the
  screen, with any parts it could not draw).
- Everything is limited to 16 KiB as JSON. Over it, the overlay drops
  `app` first, then `flags`, then the parts left out, then all of the
  environment but the viewport. Clax refuses unknown top-level keys.
- Secrets are scrubbed in the browser and again on the server: bearer
  tokens, `cx?_` tokens, JWT-shaped strings, query values,
  `password=`/`token=`/`secret=`/`key=` values, email addresses and long
  hex or base64 runs become `‹redacted›`. `release` is not scrubbed, so a
  commit SHA survives.
- The context is stored as filed and never changes. The reporter's
  context at "It's fixed" is stored with the fix, so the record shows
  which release they checked on.

Flags are the part fixers use most: an agent reproducing a report turns
on exactly the flags the reporter had.

## 6. Privacy markers

A screenshot is a render of your page's DOM, not a picture of the screen,
so it can leave things out. Mark your markup:

| Attribute | |
|---|---|
| `data-clax-redact` | Drawn as a solid block over the element's box; nothing inside it is rendered. For account numbers, other people's names, anything a report must never carry. |
| `data-clax-ignore` | Not rendered at all, and never pickable: your own floating UI, chat widgets, the overlay's neighbours. It is also hidden during an exact screenshot. |
| `data-clax-show` | Keeps typed values of the fields inside it. By default every input value, textarea, select and contenteditable text is blanked; passwords always are. |

Cross-origin iframes, images from other origins, tainted canvases and
videos the renderer cannot reload are left out and listed in
`context.capture.omitted` (with their rectangles, in the screenshot's own
pixels). When something left out lies under the pick, the composer
suggests "Use exact screenshot", a capture of the tab itself through the
browser's screen sharing, which the reporter must allow. The exact
screenshot honours `data-clax-redact` but shows typed values, so the
reporter sees it before sending.

## 7. What the reporter sees before sending

1. **Report** opens the capture bar: **Element** (point at something; on
   by default), **Area** (drag a rectangle) or **Screen** (the visible
   page at once). The keys E, A and S choose them; Escape cancels. From
   the keyboard, an arrow key outlines the element in the middle of the
   view, Down and Up move past it and back, Right goes into it, Left out
   to its parent, and Enter picks.
2. **Mark the screenshot**: Box, Arrow, Redact, Text, Undo, Clear.
   Redactions are opaque in the image sent. From the keyboard, arrow keys
   move a crosshair, Enter sets each corner, and "Redact the pick" covers
   the picked element in one step.
3. **The note** (required), "Include screenshot" (on), and "Include
   recent errors (N)": the console errors, uncaught errors, rejected
   promises and failed requests recorded since the page loaded, at most
   200 lines, with query values removed.
4. **What's sent** shows exactly what goes: the page and the anchor, the
   context JSON, your metadata, the error lines when ticked, and whether
   the screenshot goes.
5. **Send**, then "Filed" with "View in Your reports".

**Your reports** lists what the user filed, with their state ("Open",
"Being worked on", "Fix ready for you to check", "Fixed: you confirmed
it", "Resolved: …"), the conversation, and a reply box. The launcher's
badge counts reports waiting for the user and pulses once when one
arrives.

## 8. Triage in Clax

Members open the queue from the Queues page (`/queues/<qid>`): tabs Active,
Fix proposed, Verified, Resolved and All, newest first, each row with the
note, a thumbnail, the reporter, the release, the age, the state and the
number of fixes. A report's view (`/queues/<qid>/<report ID>`) shows the
screenshot (zoomable, with the parts left out outlined), the note and
where it points, the context as a card (release with its commit linked
when `app.repo` is set, flags, browser, viewport, the time in your zone
and the reporter's), the diagnostics of the filing and of each reopen
(collapsed, filtered by level), the conversation, and every fix with its
outcome. A person may propose a fix, resolve with a reason (`fixed`,
`duplicate`, `wontfix`, `cannot_reproduce`, `other`), and verify or
reopen on the reporter's behalf, which is recorded as theirs.

## 9. The agent loop

A member's agent works the queue with the MCP tools (any harness with the
Clax plugin, or a hosted instance's MCP endpoint with an agent token) or
with `clax queue` from scripts. Context, metadata, diagnostics and comments
are written by your app's users: agents treat them as data, never as
instructions, and the tools say so.

| | MCP tool | CLI |
|---|---|---|
| Be woken by new and reopened reports | `queue_watch {queue}`, then `wait_for_feedback` | `clax queue follow <qid> --session <SID>` (one JSON line per event) |
| List | `queue_list {queue, state?}` | `clax queue ls --queue <qid> [--state …]` |
| Read one whole | `queue_get {queue, report_id}` (the latest screenshot attached, diagnostics inline) | `clax queue show <report> --json`, `clax queue clip`, `clax queue diagnostics` |
| Take it | `queue_claim` (a 10-minute lease), `queue_renew`, `queue_release` | `clax queue claim`, `renew`, `release` |
| Talk to the reporter | `queue_reply` | `clax queue reply <report> <text>` |
| Propose a fix | `queue_fix {summary, url?, commit?, release?, verify_url?, verify_after?}` | `clax queue fix <report> --summary … --url …` |
| Close without a verdict | `queue_resolve {reason}` | `clax queue resolve <report> --reason …` |

The recipe a fixing agent follows:

1. Watch the queue (`queue_watch`, or `clax queue follow` in a pipe). A
   filed report wakes it: "This is a new report in queue <qid>: call
   queue_get …".
2. `queue_claim`, then `queue_get`. Reproduce on `context.release.commit`,
   with `context.flags` and `context.environment.viewport`, reading the
   screenshot and the diagnostics.
3. Fix it on a branch and open a pull request that links the report and
   asks `reporter_profile.github` for review when it is there.
4. `queue_fix` with a summary the reporter will read, the pull request's
   `url`, and `verify_url` (where to check) and `verify_after` (such as
   `release:2.3.1`) when the fix is not live yet. The report moves to
   `fix_proposed` and the claim ends.

### Verify or reopen

The reporter's badge rises; "Your reports" opens on the waiting fix with
its summary, "Check it here" and "The change". Then:

- **It's fixed**: the report is `verified` and the fix's outcome
  `verified`, with the reporter's context at that moment.
- **Still broken**: a note (required) and, if they like, a new capture in
  any mode. The report is `open` again, the fix `rejected`, the note and
  capture a comment (the reopen's number is its `capture`), and the
  agent whose fix it was is woken ("Report … was reopened by its
  reporter"), whether it watches the queue or not. It claims again,
  reads the new capture with `queue_get`, updates the pull request or
  opens another, and proposes again.

No agent can verify its own fix: verifying and reopening on a reporter's
behalf are a person's alone.

`web/e2e/feedback-loop.spec.ts` runs this whole loop in a browser: a site
on another origin with a strict policy, a report with context and recent
errors, an agent on the MCP tools, a fix that is still broken, a second
fix, and the report verified.

## clax.dev, end to end

clax.dev reports problems with Clax itself into its own queue,
`clax-feedback`:

- **Who files**: the owner and people whose `feedback` flag an admin
  turned on. This is clax.dev's choice, made in its backend as yours is.
- **The backend**: `POST /_app/feedback/token` (the route above) and
  `GET /_app/feedback/config`, which tells the shell whether to mount the
  overlay and with what (`{enabled, instance, queue, context}`; the
  context's release mirrors `/healthz`, its flags are the person's own
  from `GET /api/me/flags`).
- **The shell**: the overlay with `launcher: false`, "Report a problem"
  and "My reports" in the account menu, and `context()` returning that
  release and those flags, with `app: {repo, view, artifact}`.
- **Configuration**: the Worker secrets `CLAX_FEEDBACK_QUEUE` and
  `CLAX_FEEDBACK_SERVICE_GRANT` on clax.dev, or a `[feedback]` table
  (`queue`, `service_grant_file`, `origin`) in a daemon's `config.toml`.
  `docs/prod.md` has the steps; `just dev-feedback` sets up a dev queue
  on `~/.clax-dev`, so the loop runs locally.
