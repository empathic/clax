# Clax app: real applications (prototype)

Status: prototype, iterate quickly. Owner decisions of 2026-10-12 are in
`.superpowers/sdd/flutter-app/progress.md` (round 3) and are binding.

## Goal

Say what you want and get an application with real depth (several screens,
real features, a designed look, working logic), usable within a minute and
growing for several minutes after, on the phone with no permanent Clax UI
over it. Apps may use device features, the network and the model, each
with the owner's consent once per app.

The builder stays on the device: the Dart agent over the Messages API.

## Task contracts

Each section is one task's contract with the others. Names here are fixed;
a task that needs to change one says so in its report.

### C1. The kit (task K1)

Vendored files in `apps/clax_app/assets/kit/`, listed in `pubspec.yaml`:

- `preact.js`: one ES module with Preact, its hooks and htm, exporting `h`,
  `html`, `render`, `Fragment`, `useState`, `useEffect`, `useMemo`,
  `useRef`, `useReducer`, `useCallback`, `useContext`, `createContext`.
- `kit.css`: the design system. Tokens on `:root` with both dark-mode
  blocks of the page contract, type scale, spacing, radii, elevation, and
  component classes. Looks like a polished native mobile app, not a
  template.
- `kit.js`: Preact components over `kit.css`: `App` (safe-area shell with a
  router over `location.hash`), `AppBar`, `TabBar`, `List`, `ListItem`,
  `Card`, `Button`, `IconButton`, `Field` (text, number, date, select,
  toggle), `Sheet`, `Dialog`, `Empty`, `Stat`, `Toast`, `Chips`,
  `SegmentedControl`, `Fab`, and a `useDoc` / `useCollection` pair over the
  `db` capability that falls back to memory when `use("db")` is null.
- `chart.js`: small SVG charts (line, bar, area, pie/donut, sparkline),
  themed by the tokens.
- `dates.js`: dayjs (ESM) with relativeTime.
- `icons.js`: about 80 common icons as functions returning SVG markup.

The agent tool `add_kit` (strict, input `{parts: [...]}` from
`preact|kit|chart|dates|icons`; `kit` implies `preact`) copies the files
into the draft under `kit/`. Pages import them by relative path
(`import {html, render} from './kit/preact.js'`) and link `./kit/kit.css`.
K1 proves a module import of `./kit/preact.js` works in an app page under
the daemon's content security policy, and fixes the policy only if it must.

### C2. Tests in render (task T1)

Files `tests/*.test.js` in the draft are tests; `index.html` never loads
them. When the draft has tests, `render` runs them after the page loads,
in a second load of the draft against a scratch copy of the draft's
documents (never the person's documents, never the draft's staged writes),
and adds to its result:
`tests: {passed: N, failed: [{name, error}], skipped: N}`.

The harness, injected by the inspector, gives tests `test(name, fn)`,
`expect(v)` with `toBe`, `toEqual`, `toBeTruthy`, `toContain`,
`toBeGreaterThan`, and `ui` with `click(selectorOrText)`,
`type(selector, text)`, `text(selector?)`, `exists(selector)`,
`waitFor(fn, ms)`, `go(hash)`. A test has 5 s; the whole run 30 s.
`finish` refuses a draft whose last render had failing tests, unless the
run is out of time (then it publishes the last render with passing tests).

### C3. Plans and the build engine (task E1)

`lib/agent/plan.dart` owns `PlanStore`: one plan per app in the app
support directory, `{request, features: [{id, title, detail, status}],
updatedAt}`, status `todo|doing|done|dropped`, order is list order. It
exposes `watch(appId)`, `get`, `addFeature`, `editFeature`, `drop`,
`reorder` for the UI, and the agent tools `plan_set` (the whole list, first
plan or revision) and `plan_mark` (`{id, status}`).

The engine:
- A new app runs **first**: `plan_set`, then a usable first version of the
  core features, tested and rendered, `finish` publishes it.
- The worker then queues **continue** runs on that app until no feature is
  `todo`, the build cap is reached, or the person stops it. Each takes the
  next features in order, builds, tests, and publishes a version. The
  person's plan edits are read at the start of each continue run.
- A comment on the app is a **change** run as today; a comment that asks
  for something new adds a feature (`plan_set`) instead, when it is large.
- Caps: first 14 turns / 4 renders / 150 s / 48k output tokens; continue
  18 turns / 5 renders / 240 s / 64k; change 10 turns / 3 renders / 120 s
  / 32k; $1.50 a run and $6 a build.
- Limits of a change (owner, 2026-10-15): the change run decides first,
  from the request and the app's size, whether it fits one run; a big one
  becomes plan features (`plan_set`) and a `finish` saying so, and the
  worker builds them in continue runs (a plan with nothing left to build
  that gains features in a change run starts a new build, with its own $6;
  a build's own runs never renew it, and Resume of a capped build starts a
  new $6). A change that ends at a cap, or `give_up`s with `out_of_room`,
  gets Clax's reply naming the limit, the agent's reason under it as
  detail, keeping the failure prefix
  when nothing was published. It is kept per thread (`unfinished.json`:
  comments, the draft's edits over what the app held, the agent's notes,
  cost), and the thread offers **Keep going** (the Retry hook): a change
  run that starts from those edits where the app still holds what they
  were made on, else (the app changed) from the app, with the notes
  either way; the draft has no age limit (owner, 2026-10-16) beyond the
  day an unfinished change is kept. A change and its Keep goings share $6; each
  run's cost cap is what is left, at most $1.50.
- The system prompt is rewritten for depth: a real plan, the kit, a
  designed look, tests for logic, several screens where the app needs
  them, finishing a feature fully before the next.

### C4. Build UX (task X1, after E1)

The making screen becomes the build view: the plan as a live checklist
(doing, done, todo, dropped) with what the agent is doing now; the person
can add, edit, drop and reorder features while it builds; "Open app" as
soon as the first version lands while the build continues; the Working
chip shows the current feature over the app; a comment can be turned into
a plan feature. Stop and resume a build.

### C5. Device capabilities (task V1)

Pages call `window.claude.use(name)` for `notify`, `share`, `files`,
`camera`, `location`, declared in capabilities (`{"notify": {}}`). In the
browser (outside the Clax app) `use` resolves null.

- `notify.schedule({id, title, body, at})` (ISO time) or
  `{..., every: {hour, minute, weekdays?}}`; `cancel(id)`; `list()`. A tap
  opens the app at `#route` when given.
- `share.text(text)`, `share.file({name, mime, data})` (base64).
- `files.pick({accept})` → `{name, mime, data}`; `files.save({name, mime,
  data})`.
- `camera.take()`, `camera.pick()` → `{mime, data, width, height}`,
  downscaled to at most 1600 px.
- `location.current()` → `{lat, lon, accuracy}`.

The shell forwards each call to the Flutter host over the claxHost channel
(nonce-checked, a request ID per call); Flutter answers with a result or a
coded error (`not_granted`, `cancelled`, `unavailable`). Consent: once per
app per capability, asked by the host, kept until revoked in the app
inspector; Android runtime permissions on top.

### C6. Network (task N1)

Declared `{"net": {"hosts": ["api.example.com"]}}`. `const net = await
claude.use("net")`; `net.fetch(url, {method, headers, body})` →
`{status, headers, text(), json()}`, proxied by the daemon at `POST
/api/artifacts/{id}/net:fetch`: https only, host declared by the live
version and granted by the owner, no loopback or private addresses (after
DNS), 5 MB, 20 s. Consent once per app for its host list; the grant lives
in the daemon (migration 29). The content security policy keeps
`connect-src 'none'`. A draft (round-2 spec §5.1) holds no grants and is
never asked about: its fetches use its live app's grants, for hosts both
the draft and the live app declare, checked on each fetch; a new app's
draft gets `not_granted`.

### C7. AI in apps (task A1)

The existing `sample` capability, on the phone: the app passes the API key
to the daemon at start (never logged), consent once per app, default model
`claude-haiku-5-5`, a per-app daily cost cap (default $0.50, migration 30
if a table is needed).

### C8. App inspector (task I1)

A native screen per app, from the launcher tile's long-press menu
("Inspect"): Overview (title, ID, created, versions, size), Files (tree,
read-only code view), Data (collections → documents → JSON), Permissions
(db read grants today; device, net and AI when those tasks land, each
revocable), Plan (read-only, once E1 lands). Read-only except revoke.
