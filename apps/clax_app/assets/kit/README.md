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

`icons.js` was generated from `lucide-static/icons/*.svg`: each icon's
inner SVG elements, keyed by a camelCase name (`home` from `house.svg`,
`trash` from `trash-2.svg`), wrapped by `icon(name, {size, stroke,
label})`.
