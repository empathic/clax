## Runtime capabilities

A page reaches runtime capabilities with `await window.claude.use(name)`,
exactly as on claude.ai. The type definitions of contract 0.3.0 (claude.ai's
0.2.61, with what each artifact's own site changes) are the contract:
before writing a page, fetch the one you use from your daemon,
`<daemon_url>/_clax/contract/0.3.0/<name>.d.ts`, where `daemon_url`
comes from the `status` tool (names: `claude`, `permissions`, `artifact`,
`db`, `downloads`, `user`, `comments`, `assets`, `room`, `sample`).

Declare what the page uses in `clax.json` at its root,
`{"capabilities": {"db": {}, "user": {"scopes": ["profile"]}}}`, or pass the
inner object as `capabilities` on `publish` and Clax writes the file. The
object is the full set: passing it replaces the stored one, omitting it (and
any `clax.json`) keeps it, and `{}` clears it. A `capabilities` argument that
differs from the `clax.json` sent with it is refused (`manifest_mismatch`).
`use()` never rejects: it resolves `null` for a name the page did not declare,
for `files` and `mcp`, and outside the Clax viewer (on the artifact's own site
opened directly, `db`, `assets`, `user` and `room` still resolve), so render without the
capability first and light features up when it resolves.
`permissions` and `user` need no declaration.

A capability that reaches past the page (`net`, `sample`, `shell`, `notify`, `share`, `file`, `camera`, `location`) is in force only once the person who answers for the artifact approves (on your own Clax, its owner; on a hosted Clax, the page's maker) the version that first declares it, or a new `net` host, in Clax. Until then `use()` resolves `null` for it, and the version before keeps what was approved. Publishing never approves, whoever publishes; the publish result says what waits. Tell the person to approve it in Clax.

- `artifact` (alias `self`): `publish(html)` saves a complete document
  (starting `<!doctype html>`) as a new version, and every open view reloads
  to it. Only the person's own browser can publish; other views reject
  `not_writer`.
- `db`: shared JSON documents at paths such as `tasks/t1`, live through
  `onSnapshot`; page writes are last-writer-wins. `data/users/<id>/` is
  private to the viewer whose `user.id()` is `<id>`. Levels: agents and
  scripts with the token are `owner`, the person's browser is `admin`, a
  viewer on another machine who entered a name is `interact`, one who did
  not is `view`. On a site open to the world (`publish_public` with
  `site_access: "world"`, once the person approves), a visitor who is not
  signed in is `public`, below `view`, with a `data/users/<id>/` of their
  browser's: they read and write only where a rule names `public`, as
  `{"path": "data/guestbook", "read": "public", "write": "public"}`.
- `downloads`: `save({filename, data})` saves once the viewer accepts.
- `user`: `isOwner()`, `canEdit()`, `can(name)`, and `me()` need no
  declaration; `id()` and `profiles(ids)` need `{"user": {}}`; names and
  `search(q)` need `{"user": {"scopes": ["profile"]}}`. IDs are opaque. The
  person is one user in all their browsers on this machine: `id()` is the
  same in Chrome and Safari (and is who the CLI acts as), and `isOwner()` is
  true in each.
- `comments`: `openComposer({element})` opens the viewer's composer; with
  `{}` (not `{"composer_only": true}`) the page may also `create`, `reply`,
  `resolve`, `delete`, and `sendToClaude` as the viewer after one consent;
  `{"customAnchors": true}` lets a canvas-like page place pins itself.
- `assets`: `upload`, `list`, `delete`, in the person's browser only. An
  asset is served at `/_clax/blob/<id>` on the artifact's own site. Use
  an asset's `url` as given; to show an asset whose ID a page stored,
  `claude.assetUrl(id)` (any viewer), never `"/_blob/" + id`: assets are
  served per app, and a page loads only its own.
- `room`: `emit`, `on`, `presence`, `peers`, `onPeers`, `join`, `connected`,
  and `onConnection` reach every view of the artifact that is open now;
  nothing is stored. Presence works for every viewer. A topic declared
  `{"room": {"topics": {"<topic>": "interact"}}}` may be sent on by a viewer
  on another machine who entered a name; every other topic only by the
  person's browser.
- `sample`: `sample(input, options)`, `sample.json`, and `sample.limits` ask
  Claude with the API key configured on the person's machine, and only in
  the person's own browser: everywhere else, and when no key is configured,
  `use("sample")` resolves `null`. The first call asks the person to allow
  it, once per app: the daemon keeps the grant until the person revokes it.
  Each app may spend up to `daily_cost_cap_usd` a day (default $0.50); past
  it, calls reject `rate_limited`.
- `net` (Clax's own): `fetch(url, {method, headers, body})` resolves
  `{status, ok, url, headers, text(), json()}`, made by the daemon (the
  page itself cannot connect). Declare every host,
  `{"net": {"hosts": ["api.example.com"]}}`; only `https` URLs on those
  hosts are fetched, at most 5 MB each way and 20 s. The person is asked
  once per app for its host list. Only the person's own views get it:
  elsewhere `use("net")` resolves `null`. Types:
  `ClaxExtensions.Net` in `clax-extensions.d.ts`.
- `shell` (Clax's own): `highlight(control)` rings one of Clax's controls,
  `"comment"`, `"threads"`, `"versions"`, `"publish"`, `"fork"` or
  `"send"`, for 6 s or until the viewer presses anywhere in Clax's own bar or panels or presses Escape there (a press in the page does not reach Clax: call `highlight` again to move the ring, or let it lapse), and resolves `false` when that control is not shown to them now
  (on a phone, versions, publish and fork are in a menu); `can(control)`
  answers the same without drawing; `commentMode(on)` turns comment mode on or off, only from a click or key press in the page (while comment mode is on, a click in the page picks what to comment on and never reaches the page, so leave it from a key press; the viewer's Escape or Comment button also leaves it). Declare `{"shell": {}}`.
  Only the owner and editors get it: elsewhere `use("shell")` resolves
  `null`. Types: `ClaxExtensions.Shell` in `clax-extensions.d.ts`.
- `notify`, `share`, `file`, `camera`, `location`: Clax's device
  capabilities, served only in the Clax app and `null` everywhere else;
  their types are `ClaxExtensions.Device` in `clax-extensions.d.ts`. Call
  them from a click handler (all but `notify.list` and `notify.cancel`
  reject `no_gesture` otherwise). `location.watch` follows the device only
  while the app is on screen. See "Device capabilities in the Clax app"
  below.
- `home` (Clax's own): the Clax app's home screen. Declaring it grants
  nothing: the app serves it to the one app it shows as its home screen,
  and `use("home")` resolves `null` for every other app, in a browser and
  in the draft preview. `apps()`, `makes()`, `unread()`, `needsKey()`, the
  actions `open`, `openMake`, `newApp`, `menu`, `openInbox`,
  `openSettings`, `importApp` and `getApps` (from a click handler, as for
  the device capabilities; `getApps` only where `chrome().getApps` is
  `true`), `shown()` once the home screen is drawn, `tiles(list)`
  after each render that moves its tiles, `chrome()` (the window's form
  and the pointer in use), and `on(name, fn)`. Types: `ClaxExtensions.Home` in
  `clax-extensions.d.ts`. See "The home screen's capability" below.
- `permissions`: `state()` and `request()`; the only prompt is the consent
  to the first page-written comment.

The viewer's gesture: `openComposer` and `compose` open the composer only
within about five seconds of the viewer's input in the page, with focus in
the page and the pointer moved onto it (or a Tab into it) since their last
input to the Clax window; otherwise they resolve `{opened: false}`.
`create`, `reply`, `resolve`, `delete`, `sendToClaude`, and
`artifact.publish` also need no input to the Clax window (its buttons, name
field, composer, dialogs) in the last 5.5 seconds: inside that time they
reject `shell_input_recent` with nothing written, so show a message and let
the viewer click again. That code is Clax's own: it and Clax's other
additions are declared in `<daemon_url>/_clax/contract/clax-extensions.d.ts`,
beside the contract's files. Call these only from a click or key press in the page,
never on load or a timer.

### Differences from claude.ai

What a page written for claude.ai meets in Clax, beyond the gesture rules:

- Everywhere: nobody is a guest and there are no organizations; an
  artifact's site opened to the world is its only public view, and its
  comments stay with those it is shared with.
- `permissions`: allowing is remembered in the viewer's browser for the
  artifact, across reloads and versions. "Don't allow" lasts until the Clax
  tab is reloaded or shows another version; there is no standing refusal.
- `artifact`: `publish` needs the viewer's gesture (above); one tab may
  publish an artifact once every 2 seconds and 10 times a minute, then gets
  `rate_limited`. Called from a page other than `index.html`, it replaces
  that page's file and carries the others forward. The files form rejects
  `capability_disabled`; `edit` and `sync` reject `invalid_content`.
- `db`: no latency compensation: `hasPendingWrites` and `fromCache` are
  always `false`, and a page's own write shows in its snapshots once the
  daemon confirms it. The person's browser is `admin`, not `owner`, though
  `isOwner()` is true there: a rule at `owner` shuts out every browser,
  theirs included, so use `admin` for "only the person". The only quota is
  5000 documents per artifact.
- `user`: `can()` is fixed at its first call: after a viewer on another
  machine enters a name, it keeps its old answer until the page reloads,
  though their writes already succeed, so reload or try the write.
  `profiles()` caches names for the life of the page and never refreshes
  them. `can()` answers `null` only when the site cannot say who the viewer
  is, `email` is always `null`, and
  `search()` works only in the person's browser.
- `downloads`: at most one prompt at a time and 3 per 30 seconds;
  `request` (export answers) always rejects `request_unknown`.
- `comments`: `canSendToClaude()` answers `available` while the publishing
  agent session is live, `no_session` otherwise (never `writers_only`), and
  `off` under `composer_only`. There is no batch form of `sendToClaude`: one
  call sends one comment. `reply`, `resolve`, `delete`, and
  `sendToClaude({threadId})` act only on threads the page created in the
  current page load (`not_found` otherwise). On another machine, reopening
  and `delete` need the viewer to have entered a name (`forbidden`
  otherwise). Page-written comments show "via the page", and an `@agent` in
  them sends nothing. Per tab, composer opens are limited to 5 per 10
  seconds, page writes to 10 a minute, and gesture refusals to 20 a minute.
- `room`: there are no agent peers (`kind` is always `"viewer"`);
  `sendToClaudeSession` rejects `claude_unavailable` and
  `canSendToClaudeSession` answers `"off"`. A new version does not empty
  the room: each view stays on its version until it reloads. A viewer's
  level is fixed per connection; entering a name reconnects at the new level.
- `sample`: the person's own API key pays for every call, not the viewer's
  account, so only their browser can call it. Allowing lasts for the app
  until revoked, not for the view. Answers are cached per browser in the
  daemon and forgotten, with the day's call counts, when it restarts; the
  day's spend against the cost cap is stored and survives a restart. A daemon
  restart under an open tab ends its calls with `session_expired`.

