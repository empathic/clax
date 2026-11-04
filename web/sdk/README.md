# @empathic/clax-client-sdk

The SDK for [Clax](https://github.com/empathic/clax)'s report queues: the
people using your app file reports on its pages into a queue on your Clax
instance, and read the replies.

| Entry | What it gives | Where it runs |
|---|---|---|
| `@empathic/clax-client-sdk` | `ReporterClient`, `ClaxError` and the queue routes' types | the page |
| `@empathic/clax-client-sdk/overlay` | `overlay()`: a Report button, comment mode to point at what the report is about, and "Your reports" with their replies | the page |
| `@empathic/clax-client-sdk/service` | `ServiceClient`, which mints reporter tokens with the queue's service grant | your backend only: it holds the grant |
| `@empathic/clax-client-sdk/loader` | `install()`: Clax as a container, configured from Clax, installing only what its config names | the page |
| `@empathic/clax-client-sdk/plugins` | Clax's own plugins: capture, annotate, code blur, logs, version, browser, pickers, presentations, theme | the page |
| `@empathic/clax-client-sdk/plugin` | the plugin contract: `definePlugin`, `PluginHost` | the page |
| `@empathic/clax-client-sdk/config` | the install config: its fields, `resolve()`, its JSON Schema | anywhere |

Each entry is an ES module with its TypeScript declarations. The package
has no dependencies: Svelte and the overlay's other packages are bundled.

## Install

```
npm install @empathic/clax-client-sdk
```

## The overlay

```ts
import { overlay } from "@empathic/clax-client-sdk/overlay";

const handle = overlay({
  instance: "https://clax.example.com",
  queue: "<queue ID>",
  // Your backend mints a token for the reporter's key (below).
  auth: async publicKey => {
    const res = await fetch("/clax/reporter-token", { method: "POST", body: publicKey });
    return res.json(); // { token, expires_at }
  },
});

// Later: remove it and end comment mode.
handle.close();
```

The same entry exports `forgetReporter({ instance, queue, user })`, which
deletes the key and token kept for a reporter (call it on sign-out),
`diagnostics.install()`, which starts recording the page's recent errors
before the overlay does (nothing leaves the page unless the reporter ticks
"Include recent errors"), and `CONTEXT_TIMEOUT_MS`, how long a report waits for the `context` option's
answer before filing without it.

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

const client = new ReporterClient({ instance: "https://clax.example.com", queue: "<queue ID>", auth });
try {
  await client.fileReport({ url: location.href, title: document.title, anchor: null, body: "The total is wrong" });
  const { reports, next } = await client.myReports();
} catch (e) {
  if (e instanceof ClaxError) console.error(e.code, e.status);
}
```

## The backend

```ts
import { ServiceClient } from "@empathic/clax-client-sdk/service";

// The queue's service grant (its `token` and `private_key`), from your secrets.
const service = new ServiceClient({ instance: "https://clax.example.com", queue: "<queue ID>", token: grant.token, privateKey: grant.private_key });
// For the signed-in user, from the page's origin, for the key the page sent:
const token = await service.mintReporterToken({ reporter_id: user.id, origin: "https://app.example.com", public_key: publicKey });
```

## License

LGPL-2.1-only: see `LICENSE` and `LICENSES/LGPL-2.1-only.txt`. The packages
it bundles keep their own licenses, which `THIRD-PARTY-NOTICES.txt` holds.
