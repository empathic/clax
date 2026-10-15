# The Clax kit

The files the `add_kit` tool (`lib/agent/tools/kit_tool.dart`) copies into
an app's draft under `kit/`. `example/index.html` is a sample app over all
of them; it is not bundled (Flutter bundles only this directory's own
files).

| File | What | Source and license |
|---|---|---|
| `preact.js` | Preact, its hooks and htm as one ES module | preact 10.29.8 (MIT), htm 3.1.1 (Apache-2.0) |
| `dates.js` | dayjs with the relativeTime plugin, as an ES module | dayjs 1.11.23 (MIT) |
| `icons.js` | 109 icons as functions returning SVG markup | Lucide, lucide-static 0.460.0 (ISC) |
| `kit.css` | the design system: tokens, type, components | Clax |
| `kit.js` | Preact components over `kit.css` | Clax |
| `chart.js` | SVG line, area, bar, pie, donut and sparkline charts | Clax |
| `map.js` | `MapView`, a slippy map: tiles fetched through the `net` capability and shown as `blob:` images, markers, a "you are here" dot; its styles are added by the module | Clax |

The map loads nothing itself: a page may reach other hosts only through
`net` (docs/contract.md, "Fetching from the network"), so `map.js` fetches
each `{z}/{x}/{y}` tile with `net.fetch`, keeps up to 300 as `blob:` URLs
(least recently used are revoked), runs 6 fetches at a time and never
starts one for a tile no map shows any more. The app declares the tile host
(`{"net": {"hosts": ["tile.openstreetmap.org"]}}`); the first tile asks
the owner, and a refusal shows a note in the map instead of tiles. In the
app's inspect view (the agent's renders), where nobody is asked, a map
without tiles draws a grey grid captioned "Map preview" instead. A failed
tile is tried again after 1 s, then twice as long each time, up to 15 s.
OpenStreetMap's tile policy wants its attribution visible, an identifying
client and cached tiles: the map always shows it, the daemon sends
`User-Agent: Clax/<version> (+https://github.com/empathic/clax)`, and the
daemon's HTTP cache keeps tiles by their cache headers, per app.

An app that declares `{"location": {}}` gets a "Show my location" button
on the map (`locate: false` hides it): a tap starts `location.watch`
(every 10 m), the map centres on the first position and draws a dot with
its accuracy circle, or for an approximate position only the area's
circle, zoomed to show it. The watch ends on a second tap or when the map
unmounts; a `stopped` end leaves the button to tap again, and a refusal
shows a short message.

Each vendored file opens with a comment naming its packages, versions,
licences and copyright holders; `LICENSES.txt` holds the full licence
texts and `add_kit` copies it with every vendored file.

## How the vendored files were built

In a scratch directory (`target/kit-build`), with esbuild 0.25.12:

```sh
npm install preact@10.29.8 htm@3.1.1 dayjs@1.11.23 lucide-static@0.460.0
```

`preact-entry.js`:

```js
import { h, render, Fragment, createContext } from 'preact';
import { useState, useEffect, useMemo, useRef, useReducer, useCallback, useContext, useLayoutEffect } from 'preact/hooks';
import htm from 'htm';
const html = htm.bind(h);
export { h, html, render, Fragment, createContext, useState, useEffect, useMemo, useRef, useReducer, useCallback, useContext, useLayoutEffect };
```

`dates-entry.js`:

```js
import dayjs from 'dayjs';
import relativeTime from 'dayjs/plugin/relativeTime.js';
dayjs.extend(relativeTime);
export default dayjs;
export { dayjs };
```

```sh
esbuild preact-entry.js --bundle --format=esm --minify --legal-comments=none --outfile=preact.js
esbuild dates-entry.js --bundle --format=esm --minify --legal-comments=none --outfile=dates.js
```

The header comments were added to the built files, and `LICENSES.txt`
joins each package's `LICENSE` file. `icons.js` was generated from `lucide-static/icons/*.svg`: each icon's
inner SVG elements, keyed by a camelCase name (`home` from `house.svg`,
`trash` from `trash-2.svg`), wrapped by `icon(name, {size, stroke,
label})`.
