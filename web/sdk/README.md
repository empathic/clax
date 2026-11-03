# @empathic/clax-sdk

The SDK for [Clax](https://github.com/empathic/clax)'s report queues: the
people using your app file reports on its pages into a queue on your Clax
instance, and read the replies.

| Entry | What it gives | Where it runs |
|---|---|---|
| `@empathic/clax-sdk` | `ReporterClient`, `ClaxError` and the queue routes' types | the page |
| `@empathic/clax-sdk/overlay` | `overlay()`: a Report button, comment mode to point at what the report is about, and "Your reports" with their replies | the page |
| `@empathic/clax-sdk/service` | `ServiceClient`, which mints reporter tokens with the queue's service grant | your backend only: it holds the grant |
| `@empathic/clax-sdk/audit` | `AuditClient`, which reads the instance's audit trail: a page, one event, or new events as they are recorded | the owner's own tools (the daemon token) or the instance's own pages (a browser session); agents and grants are refused |

Each entry is an ES module with its TypeScript declarations. The package
has no dependencies: Svelte and the overlay's other packages are bundled.

## Install

```
npm install @empathic/clax-sdk
```

## The overlay

```ts
import { overlay } from "@empathic/clax-sdk/overlay";

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

## The reporter client

```ts
import { ClaxError, ReporterClient } from "@empathic/clax-sdk";

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
import { ServiceClient } from "@empathic/clax-sdk/service";

// The queue's service grant (its `token` and `private_key`), from your secrets.
const service = new ServiceClient({ instance: "https://clax.example.com", queue: "<queue ID>", token: grant.token, privateKey: grant.private_key });
// For the signed-in user, from the page's origin, for the key the page sent:
const token = await service.mintReporterToken({ reporter_id: user.id, origin: "https://app.example.com", public_key: publicKey });
```

## The audit trail

```ts
import { AuditClient } from "@empathic/clax-sdk/audit";

// The owner by hand: the daemon token (without one, the browser's session).
const audit = new AuditClient({ instance: "http://localhost:7480", token: process.env.CLAX_TOKEN });
const { events, next_cursor } = await audit.list({ kind: "bot.*", from: "2026-11-01" });
const detail = await audit.event(events[0].seq); // with its stored body
for await (const e of audit.follow({ group: "changes" })) console.log(e.seq, e.kind, e.summary);
```

Each reader sees its own scope: the owner and admins every event, a
connected person the events theirs to read. An agent's or a grant's request
is refused (403 `not_for_agents`, `forbidden`).

## License

LGPL-2.1-only: see `LICENSE` and `LICENSES/LGPL-2.1-only.txt`. The packages
it bundles keep their own licenses, which `THIRD-PARTY-NOTICES.txt` holds.
