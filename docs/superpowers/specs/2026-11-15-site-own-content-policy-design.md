# A clax that sets its own content policy

## Why

Every response on a clax's site carries the lockdown
(`crates/clax-http/src/content_policy.rs`, `lockdown`): the page loads and
connects to its own site only, posts no form, and registers no service
worker. That is the right rule for a page an agent just published and
nobody has vouched for: an injected page cannot send what its viewer can
read to another host.

It is the wrong rule for a clax that is a web app in its own right, which
may be the primary site of whoever runs it. Such a site loads scripts,
styles and fonts from CDNs, embeds frames, posts forms, opens WebSockets
to its own services (iroh's relays, for one) and may install a service
worker. No web host forbids these, and the `net` capability does not
replace them: it carries HTTPS requests and answers through the daemon,
never a WebSocket, a `<script src>` or a frame.

## What changes

One switch per artifact, `content_policy`, with two values:

- `locked` (the default, and every existing artifact's): today's
  behaviour, unchanged.
- `own`: the clax is trusted as a site. Clax stops deciding what its
  pages load, connect to, post to and register; the page's own `<meta>`
  policy, if it has one, is the only content policy besides framing.

The switch belongs to the artifact, not to a version: it says the clax is
trusted, and later versions keep it. Whoever administers the artifact may
return it to `locked` at any time.

## Goals

- A clax set to `own` can do what an ordinary site can: third-party
  scripts, styles, fonts, images, media and frames; `fetch`,
  `XMLHttpRequest`, `WebSocket` and `EventSource` to any host; form posts;
  a `<base>`; a service worker of its own (where it may register: below).
- A clax nobody switched behaves exactly as today.
- Only the person who answers for the artifact can switch it to `own`,
  through the same gate as opening its site to the world.

## Non-goals

- Per-origin allowlists (a list of hosts per directive). One switch says
  what is being decided; a list of CDN hosts tells the person approving
  little and is fiddly for an agent to maintain.
- Response headers chosen by the clax (a `_headers` file, a policy sent as
  a header). A page that wants a policy of its own writes a `<meta>` one.
- Custom domains for a clax's site.
- Any change to `Referrer-Policy`, `X-Content-Type-Options`,
  `Cross-Origin-Resource-Policy` or `Strict-Transport-Security` on site
  responses, or to the `net` capability.

## Anti-goals

- A page, an agent or a publish must never switch a clax to `own` on its
  own: a prompt-injected page lifting its own lockdown defeats the
  default.
- `own` must not loosen what protects Clax itself: who may frame the
  site, and the rules of `/_clax/`.

## Behaviour

### The setting

- Stored on the artifact: a column `content_policy TEXT NOT NULL DEFAULT
  'locked' CHECK (content_policy IN ('locked','own'))`, in the next free
  migration.
- Read: `GET /api/artifacts/<id>` and every answer that carries
  `site_access` today carries `content_policy` beside it (additive).
- Written: `PATCH /api/artifacts/<id> {"content_policy": "own"}`
  (`"locked"` returns it; anything else is 400 `invalid_argument`), under
  the rules `site_access` has in the same route (`docs/contract.md`,
  "A site is private until…"):
  - whoever administers the artifact may send it (the page's maker, and on
    a local Clax the owner); anyone else gets 403 `admin_only`;
  - a page (`x-clax-via: page`) never may;
  - an agent's switch to `own` waits on its person: 202
    `approval_pending` with a question the daemon writes (`Let it set its
    own content policy` or `Keep it locked`), then the call again with
    `question_id`, exactly as an agent's opening of a site does; an
    approval is used once;
  - switching to `own` is a confirmed decision where confirmations are
    required, in the group that widens who may open a page (decision 3 of
    the list in `docs/contract.md`); returning to `locked` never asks.
- Either change holds from the next request.
- The change is recorded as `artifact.update` of the artifact, as
  `site_access` is.
- The setting does not travel: a push to a public Clax, a fork, a clone
  and a bundle each start `locked`. The public copy's maker sets it there.
- A draft follows its app's setting, as it follows its app's capability
  approval. A new app's draft, with no live app, is `locked`.

### What `own` lifts

On a site whose artifact is `own`:

- **Content responses** (the clax's own pages and files, at the root and
  under `/v/<n>/`, and a draft's) carry
  `Content-Security-Policy: <frame-ancestors as today>` and nothing more.
  Today these carry the lockdown after the same `frame-ancestors`
  (`crates/clax-server/src/routes/content.rs`, `content_policy`).
- **A service worker's script** (`Service-Worker: script`) is served from
  the clax's files like any other file, not answered with the kill switch
  (`crates/clax-http/src/host.rs`, `site_layer`).

What stays, on every site:

- `frame-ancestors`: who may frame the site is Clax's rule, and a `<meta>`
  policy cannot set it.
- A live page's snapshot keeps its snapshot policy: the snapshot is
  Clax's rendering of another site, not the clax's own code.
- Every response that is not the clax's own content (`/_clax/…`, a 404, a
  redirect, a refusal) keeps the lockdown: those are Clax's documents.
- The other isolation headers, unchanged.
- The gated capabilities (`net` and the rest) keep their approval and
  grants: `own` lifts the browser's policy, not what the daemon will do
  on a page's behalf.

### Returning to `locked`

Pages loaded from the next request carry the lockdown again, and so does
every other file a browser kept from while the clax was `own`: while `own`,
its content is revalidated on every use (none of it `immutable`, as a
version's raw files otherwise are), and each `ETag` also names the policy,
so a browser's next use after relocking is answered `200` with the
lockdown, never `304` (a `304` would keep the stored policy). Without
this, a document-capable file (SVG, XHTML, XML) cached for a year without
the lockdown lets a locked page frame it and fetch anywhere through it. A
file a browser kept `immutable` from while the clax was `locked` keeps the
lockdown until it expires, after a switch to `own` too: stricter than the
setting, so accepted.

A service worker the clax registered while `own` is answered with the
kill switch at its next update check and unregisters; until then it still
controls the tabs it already controls, and can answer them with documents
that carry no policy. This window is accepted, and recorded as a waiver on
`site_layer`.

### Where its service worker registers

A worker's script request is decided by the site's access like any other
request, so a private site answers it only with a site session: an `own`
clax's worker registers on a site open to the world, or on a private site
visited top-level on a hosted Clax, whose first-party site cookie the
script request carries. In Chromium and Firefox it does not register from
inside the shell's frame of a private site: they send the frame's
partitioned site cookie with the frame's own fetches but not with its
worker's script fetch, which is answered 401 `site_session_required`. Nor,
there, on a local Clax's private site, whose top-level visit is sent to the
shell. WebKit sends the frame's site cookie with the script request too, so
there it registers from the shell's frame of a local Clax's private site. In the
Clax app it never registers: a content origin there is `clax://<id>.c` on
Apple platforms, a scheme no service worker is registered on, and
`https://<id>.c.clax.invalid` on Android, whose worker script fetch the app
does not answer (it intercepts its WebViews' requests, not a service
worker's). Serving a worker's script
without a session would hand a private site's files to anyone who sends
`Service-Worker: script`.

### Where a person sees it

- The switch sits beside "open to the world" wherever the shell offers
  that today, for a caller who may administer the artifact. Its label:
  "Let this clax set its own content policy". Its explanation, shown
  before the change is sent: "Its pages may then load code from, and send
  anything they can read to, any site. So may every later version,
  whoever publishes it: you, an editor, or an agent of either. Turn this
  on for a clax you trust as a website."
- An artifact that is `own` shows it wherever the shell shows that a site
  is open to the world.

### Where an agent meets it

- The MCP tool that carries `site_access` (`publish_public`) takes
  `content_policy` the same way, making the same waiting call.
- The page contract's rule ("A page loads and fetches only its own files…
  no scripts, styles, fonts or images from CDNs…") becomes the default,
  and says what to do otherwise: bundle the files with the page, or ask
  the person to let the clax set its own content policy.

## What the person accepts with `own`

Recorded as waivers beside the code that implements each:

- A page of the clax can send anything it can read (the clax's database
  and assets, who the viewer is, the room) to any host. Every later
  version runs unlocked too, whoever publishes it: the maker, an editor
  (on a hosted Clax, anyone with Can edit), or an agent of either; and so
  does a draft of it.
- Viewers' browsers contact whatever hosts the page names.
- A service worker of the clax's can answer its own site's requests,
  those under `/_clax/` in its scope included.

## Left to the builder

- The exact place the setting is resolved for a response (the content
  route already loads the artifact; the service worker branch of
  `site_layer` needs one lookup, only for such a request).
- How the Worker's site hosts reach the setting; the daemon and the Worker
  must answer alike.
- The shell component and wording details beyond the label and the
  explanation above.

## Tests

- Unit: the policy of an `own` content response is its `frame-ancestors`
  alone; a `locked` one is unchanged.
- Integration (`crates/clax-server/tests`): the PATCH's rules (admin only,
  never a page, an agent's waiting call, used once, confirmation where
  required, `locked` never asks); the headers of a page, a `/v/<n>/` file,
  a draft, a 404 and a `/_clax/` response under each setting; the service
  worker's script under each setting and after returning to `locked`; a
  fork and a push start `locked`.
- End to end (`web/e2e`, beside `lockdown.spec.ts`): a page that loads a
  script from another origin and opens a WebSocket to it is refused while
  `locked` and works once the owner switches the clax to `own`; the switch
  in the shell shows its explanation first.
