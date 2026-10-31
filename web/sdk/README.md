# @empathic/clax-sdk

The SDK for [Clax](https://github.com/empathic/clax)'s report queues: the
people using your app file reports on its pages into a queue on your Clax
instance, and read the replies.

| Entry | What it gives | Where it runs |
|---|---|---|
| `@empathic/clax-sdk` | `ReporterClient`, `ClaxError` and the queue routes' types | the page |
| `@empathic/clax-sdk/overlay` | `overlay()`: a Report button, comment mode to point at what the report is about, and "Your reports" with their replies | the page |
| `@empathic/clax-sdk/service` | `ServiceClient`, which mints reporter tokens with the queue's service grant | your backend only: it holds the grant |
| `@empathic/clax-sdk/anchor` | anchors: build one from an element, a text range or an area, resolve it on a later load, and `CommentMode` to pick one | the page |

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

### Theming the overlay

The overlay draws in Clax's design tokens unless the page passes its own:
a token file in the [W3C Design Tokens format](https://www.designtokens.org/tr/drafts/format/)
with `theme.light`, `theme.dark` and `base` groups, each optional, in the
roles of Clax's token file (`bg`, `card`, `fg`, `muted`, `border`,
`accent`, `you`, `agent`, `danger`, `font`, `radius` and the rest; the
full set is `tokens/clax.tokens.json` in the repository). A role the page
leaves out keeps Clax's. A broken file (an alias to no token, a value that
is not a colour) throws before the overlay is added.

```ts
overlay({
  instance, queue, auth,
  theme: {
    palette: { $type: "color", paper: { $value: "#fdf6e3" }, ink: { $value: "#073642" }, violet: { $value: "#6c71c4" } },
    theme: { light: { bg: { $value: "{palette.paper}" }, fg: { $value: "{palette.ink}" }, agent: { $value: "{palette.violet}" } } },
    base: { radius: { $type: "dimension", $value: { value: 2, unit: "px" } } },
  },
});
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

## Anchors

An anchor is how a comment holds on to what it is about: its element's
selector and fingerprint, the quoted text with what comes before and after
it, or an area as fractions of an element's box. It survives a reload and
most edits to the page.

```ts
import { buildElementAnchor, buildRangeAnchor, resolveAnchor, placeArea, rectOf } from "@empathic/clax-sdk/anchor";

const anchor = buildElementAnchor(document, someElement);
// Or from what the person selected:
const quoted = buildRangeAnchor(document, getSelection()!.getRangeAt(0));

// Later, on a fresh load of the page:
const found = resolveAnchor(document, anchor); // { method, element, range } or null
if (found) {
  // "exact", "selector", "quote" or "custom": how it was found.
  const box = anchor.area ? placeArea(anchor, found.element) : rectOf(found.range ?? found.element);
}
```

`CommentMode` is the pointer interaction the overlay uses to pick what a
comment is about: it outlines the target under the pointer and reports the
element, range or dragged area the person chooses.

## License

LGPL-2.1-only: see `LICENSE` and `LICENSES/LGPL-2.1-only.txt`. The packages
it bundles keep their own licenses, which `THIRD-PARTY-NOTICES.txt` holds.
