# @empathic/clax-client-sdk

The SDK for [Clax](https://github.com/empathic/clax)'s report queues: the
people using your app file reports on its pages into a queue on your Clax
instance, and read the replies.

| Entry | What it gives | Where it runs |
|---|---|---|
| `@empathic/clax-client-sdk` | `ReporterClient`, `ClaxError`, the queue routes' types, and `loadOverlay`/`reporterTokenFrom` for a page without a bundler | the page |
| `@empathic/clax-client-sdk/overlay` | `overlay()`: a Report button, comment mode to point at what the report is about, and "Your reports" with their replies | the page |
| `@empathic/clax-client-sdk/service` | `ServiceClient` and `ClaxError`: it mints reporter tokens with the queue's service grant | your backend only: it holds the grant |
| `@empathic/clax-client-sdk/loader` | `install()`: Clax as a container, configured from Clax, installing only what its config names | the page |
| `@empathic/clax-client-sdk/plugins` | Clax's own plugins: capture, annotate, code blur, logs, version, browser, pickers, presentations, theme | the page |
| `@empathic/clax-client-sdk/plugin` | the plugin contract: `definePlugin`, `PluginHost` | the page |
| `@empathic/clax-client-sdk/config` | the install config: its fields, `resolve()`, its JSON Schema | anywhere |
Each entry is an ES module with its TypeScript declarations; the
`/service` entry is also CommonJS (`require`), for a backend on any Node
20. The package has no dependencies: Svelte and the overlay's other
packages are bundled.

The whole integration, from making the queue to triage, is the guide
[Bug reports from your app to your agents](https://github.com/empathic/clax/blob/main/docs/feedback.md);
the routes, fields, limits and error codes are in
[the contract's "Report queue routes"](https://github.com/empathic/clax/blob/main/docs/contract.md#report-queue-routes).

## Before you start

On your Clax instance's Queues page (`/queues`), as the queue's admin:

1. **New queue**: name it, and list the origins your pages are served
   from (`https://app.example.com`). Its ID is the `queue` below.
2. **Manage, Create service grant**: copy its token and private key into
   your backend's secrets. They are shown once, and never go to a browser.

## Install

```
npm install @empathic/clax-client-sdk
```

## The overlay

```ts
import { overlay } from "@empathic/clax-client-sdk/overlay";

const handle = overlay({
  // The instance's origin: Clax is served at the root of its own host.
  instance: "https://clax.example.com",
  queue: "<queue ID>",
  // The signed-in user's stable ID: the reporter's key and token are kept
  // across page loads, so your backend mints a token about once per 15
  // minutes rather than on every page load.
  user: currentUser.id,
  // Your backend mints a token for the reporter's key (below).
  auth: async publicKey => {
    const res = await fetch("/clax/reporter-token", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ public_key: publicKey }),
    });
    if (!res.ok) {
      // The route's code rides on the error: on `invalid_key` the overlay
      // drops its kept key and asks once more with a fresh one.
      const code = (await res.json().catch(() => null))?.error?.code;
      throw Object.assign(new Error(`no reporter token: ${code ?? res.status}`), { code });
    }
    return res.json(); // { token, expires_at }
  },
});

// Later: remove it and end comment mode.
handle.close();
```

Its other options (`launcher`, `canFile`, `context`, `metadata`,
`diagnostics`, `externalId`, `nonce`) are in the guide's
["The overlay's options"](https://github.com/empathic/clax/blob/main/docs/feedback.md#4-the-overlays-options).
If your app has a page of its own listing the user's reports, the
overlay can send them there instead of its "Your reports" panel:

```ts
overlay({
  // …the options above, and:
  launcher: false,
  // What the Filed notice calls your page: "Replies and fixes show in
  // Feedback." and its button, "View in Feedback". Default "Your reports".
  reportsLabel: "Feedback",
  // Called by that button with the report just filed, and by the corner's
  // "Your reports" (with `launcher: "corner"`) with none. Default: none,
  // and both open the panel. `handle.showReports()` still opens the panel.
  onShowReports: report => router.push(report ? `/feedback#${report.id}` : "/feedback"),
});
```

The same entry exports `forgetReporter({ instance, queue, user })`, which
deletes the key and token kept for a reporter (call it on sign-out),
`diagnostics.install()`, which starts recording the page's recent errors
before the overlay does (nothing leaves the page unless the reporter ticks
"Include recent errors"), and `CONTEXT_TIMEOUT_MS`, how long a report waits for the `context` option's
answer before filing without it.

The overlay's lifecycle — loading, the reporter's reports and their
states, and the count waiting on a verdict or an unread reply — is kept
apart from its UI: a host with `launcher: false` and its own button reads
it with `handle.subscribe`, a Svelte store (`subscribe(fn) =>
unsubscribe`, called at once with the current state):

```ts
const handle = overlay({ instance, queue, auth, launcher: false });
handle.subscribe(state => {
  myOwnButton.dataset.needsReview = state.waiting ? "true" : "false";
});
```

## Loading the overlay from a script tag

A page that cannot `import` the `./overlay` entry (no bundler, or the
overlay must load from the instance itself, never pinned into your build)
loads it as a classic script and gets `window.Clax` back, typed — in
place of hand-typing `window.Clax` and writing your own script injector:

```ts
import { loadOverlay, reporterTokenFrom } from "@empathic/clax-client-sdk";

const Clax = await loadOverlay("https://clax.example.com");
const handle = Clax.overlay({
  instance: "https://clax.example.com",
  queue: "<queue ID>",
  auth: reporterTokenFrom("/clax/reporter-token"), // POSTs {public_key}, reads {token, expires_at}
  launcher: false,
});
```

`reporterTokenFrom(url, { fetch?, headers? })` is the client side of the
token route (above, "The backend"): it forwards `trace.traceparent` as a
header when the overlay gives one, and throws with `code` read from a
`{ error: { code } }` body on a non-OK answer, so the overlay's drop-and-
retry on `invalid_key` keeps working. Write your own `auth` instead when
your route's shape differs.

## Plugins

Every feature the overlay shows is a plugin: how a screenshot is taken,
the drawing tools, privacy filters, attachments, how a report points at the
page, and how the composer is presented. `overlay()` without `plugins` runs
`classic()`; pass your own list to choose:

```ts
import { overlay } from "@empathic/clax-client-sdk/overlay";
import { annotate, appVersion, browser, codeBlur, domCapture, displayCapture, elementPicker, logs, pagePicker, presentations } from "@empathic/clax-client-sdk/plugins";

overlay({
  instance, queue, auth,
  presentation: "dialog",
  theme: { "--clax-you": "#6d28d9" },
  plugins: [domCapture(), displayCapture(), annotate(), codeBlur({ selector: "pre, .cm-content" }),
    logs({ source: () => myLog.lines() }), appVersion({ release: { version: "1.4.0" } }), browser(),
    elementPicker(), pagePicker(), presentations()],
});
```

A plugin that adds data to a report must give its line in "What's sent";
data without one is not sent. Write your own with `definePlugin` from
`@empathic/clax-client-sdk/plugin`.

## Install as a container

`install()` reads the site's config from Clax (the container), lays your
defaults under it and a viewer's override over it, and installs only what
the result names: a site with screenshots off never loads capture.

```ts
import { install } from "@empathic/clax-client-sdk/loader";

const clax = await install({
  instance: "https://clax.example.com",
  container: "<queue ID>",
  auth: "/clax/reporter-token",          // or a function, as for overlay()
  site: { presentation: "dialog", start: "note", entry: { launcher: false, key: "f" } },
  logs: () => myLog.lines(),
  release: { version: "1.4.0" },
});
clax.source.presentation; // "clax", "site", "you" or "default"
```

Any element starts Clax with `data-clax="report"` (or `"reports"`); so do
the config's `targets` selectors, its key, a `?clax=report` link, and
`clax.push(["report"])`. With the classic script, a tag manager queues
`window.clax = [["install", {...}]]` before `/_clax/overlay.js` loads.
The config's fields and their JSON Schema are in `@empathic/clax-client-sdk/config`
(`clax.config.schema.json`).

## The reporter client

```ts
import { ClaxError, ReporterClient } from "@empathic/clax-client-sdk";

const client = new ReporterClient({ instance: "https://clax.example.com", queue: "<queue ID>", user: currentUser.id, auth });
try {
  await client.fileReport({ url: location.href, title: document.title, anchor: null, body: "The total is wrong" });
  const { reports, next } = await client.myReports();
} catch (e) {
  if (e instanceof ClaxError) console.error(e.code, e.status, e.retryAfterS);
}
```

Both clients retry as Clax's signing clients do: once on 401
`stale_request` with the instance's clock, and three times on 429
`limit_reached` and 503 `busy`, after 1, 2 and 4 seconds. The reporter
client asks `auth` for a new token on 401 `token_expired` or
`invalid_token` alone. `ClaxError.code` is typed `ClaxErrorCode`, the
codes the queue routes and the SDK answer.

## The backend

The route behind your own sign-in that the overlay's `auth` calls:

```ts
import { ClaxError, ServiceClient } from "@empathic/clax-client-sdk/service";
// From CommonJS: const { ClaxError, ServiceClient } = require("@empathic/clax-client-sdk/service");

// The queue's service grant (its `token` and `private_key`), from your secrets.
const service = new ServiceClient({ instance: "https://clax.example.com", queue: "<queue ID>", token: grant.token, privateKey: grant.private_key });

app.post("/clax/reporter-token", async (req, res) => {
  const user = req.session.user; // your sign-in decides who may report
  if (!user) return res.status(401).json({ error: { code: "sign_in", message: "sign in first" } });
  const { public_key } = req.body; // the page's JSON body
  try {
    // For the signed-in user, from the page's origin, for the key the page sent:
    const token = await service.mintReporterToken({ reporter_id: user.id, origin: "https://app.example.com", public_key });
    res.set("cache-control", "no-store").json(token); // { token, expires_at }
  } catch (e) {
    // A refused key, or a limit still reached after the client's retries:
    // passed back with its code, which the overlay acts on.
    if (e instanceof ClaxError && (e.code === "invalid_key" || e.code === "limit_reached")) {
      if (e.retryAfterS !== undefined) res.set("retry-after", String(e.retryAfterS));
      return res.status(e.status).json({ error: { code: e.code, message: e.message } });
    }
    res.status(503).json({ error: { code: "feedback_unavailable", message: "reports are off for now" } });
  }
});
```

## License

LGPL-2.1-only: see `LICENSE` and `LICENSES/LGPL-2.1-only.txt`. The packages
it bundles keep their own licenses, which `THIRD-PARTY-NOTICES.txt` holds.
