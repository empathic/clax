# @empathic/clax-sdk

The SDK for [Clax](https://github.com/empathic/clax)'s report queues: the
people using your app file reports on its pages into a queue on your Clax
instance, and read the replies.

| Entry | What it gives | Where it runs |
|---|---|---|
| `@empathic/clax-sdk` | `ReporterClient`, `ClaxError` and the queue routes' types | the page |
| `@empathic/clax-sdk/overlay` | `overlay()`: a Report button, comment mode to point at what the report is about, and "Your reports" with their replies | the page |
| `@empathic/clax-sdk/service` | `ServiceClient`, which mints reporter tokens with the queue's service grant | your backend only: it holds the grant |

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

## License

LGPL-2.1-only: see `LICENSE` and `LICENSES/LGPL-2.1-only.txt`. The packages
it bundles keep their own licenses, which `THIRD-PARTY-NOTICES.txt` holds.
