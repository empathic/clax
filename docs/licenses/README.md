# Which license applies where

Clax is licensed under **GPL-2.0-only** or **AGPL-3.0-only**, part by part.
Two small parts that are made to be copied into other people's software
carry a more permissive license, and that license applies to nothing else.
A license text being in this directory does not put Clax as a whole under
it: the table below says what each one covers.

"Only" means no later version of the GNU licenses applies.

| Path | License | Text |
|---|---|---|
| Every file not listed in another row, including `scripts/`, `demo/` and `docs/` | GPL-2.0-only | [GPL-2.0-only.txt](GPL-2.0-only.txt) |
| `crates/clax-cli`, `crates/clax-commands`, `crates/clax-hooks`, `crates/clax-embedded`, `crates/clax-fake-exe` | GPL-2.0-only | [GPL-2.0-only.txt](GPL-2.0-only.txt) |
| `plugins/` (the agent plugins, `plugins/pi` among them) | GPL-2.0-only | [GPL-2.0-only.txt](GPL-2.0-only.txt) |
| `web/extension/` (the browser extension) | GPL-2.0-only | [GPL-2.0-only.txt](GPL-2.0-only.txt) |
| `apps/clax_app` (the Clax app and its home app), except `apps/clax_app/assets/kit` | GPL-2.0-only | [GPL-2.0-only.txt](GPL-2.0-only.txt) |
| `crates/clax-core`, `clax-store`, `clax-sample`, `clax-realtime`, `clax-http`, `clax-server`, `clax-mcp`, `clax-rt`, `clax-git`, `clax-content-git`, `clax-bot-sandbox`, `clax-bot-js`, `clax-testkit` (code the local programs and the hosted service share) | GPL-2.0-only **or** AGPL-3.0-only, at your choice | [GPL-2.0-only.txt](GPL-2.0-only.txt), [AGPL-3.0-only.txt](AGPL-3.0-only.txt) |
| `crates/clax-worker`, with `crates/clax-worker/vfs` (the hosted service) | AGPL-3.0-only | [AGPL-3.0-only.txt](AGPL-3.0-only.txt) |
| `web/`, except `web/extension/`, `web/sdk/`, `web/overlay/` and `web/bridge/` (the web application) | AGPL-3.0-only | [AGPL-3.0-only.txt](AGPL-3.0-only.txt) |
| `web/sdk/`, `web/overlay/`, `web/bridge/` **only** (the SDK and overlay another product embeds, and the bridge code they are built from) | LGPL-2.1-only | [LGPL-2.1-only.txt](LGPL-2.1-only.txt) |
| Clax's own files in `apps/clax_app/assets/kit` **only**: `kit.css`, `kit.js`, `chart.js`, `map.js`, `fake-home.js` (the starter kit the app copies into the apps people make with it) | 0BSD | [0BSD.txt](0BSD.txt); also in `apps/clax_app/assets/kit/LICENSES.txt` |
| The vendored files in `apps/clax_app/assets/kit`: `preact.js`, `dates.js`, `icons.js` | Their own (MIT, Apache-2.0, ISC) | `apps/clax_app/assets/kit/LICENSES.txt` |
| `third_party/clax-rquickjs-sys`, `third_party/clax-rquickjs-core` (patched copies of third-party crates) | MIT | their manifests, and `third_party/clax-rquickjs-sys/quickjs/LICENSE` |

The repository's [`LICENSE`](../../LICENSE) holds the same list and is the
one that governs; keep the two in step when a part is added or moved.

A package's own metadata (the `license` field of its `Cargo.toml` or
`package.json`) names the same license as its row. Copies of these texts
that ship with a part are checked against this directory: the Clax app's
`apps/clax_app/LICENSE` and `plugins/pi/LICENSE` against the GPL-2.0 text,
and the published SDK package's `LICENSES/LGPL-2.1-only.txt` against the
LGPL-2.1 text.
