# Apps on clax.dev: push from the Clax app, pull into it

Date: 2026-10-21. Status: the owner's decisions (section 3) are settled.
This document awaits the owner's review. Points marked *My call* were
settled by the author of this document and are open to the owner's
change.

## 1. Intent

The Clax app makes small personal apps ("say a thing, get an app, improve it
as you use it"). Today an app leaves a device only as a signed `.clax` file
(`docs/contract.md`, "Sharing apps"). A recipient gets updates only when
someone sends them a later file. Your second device cannot take over
authorship of an app you made on the first.

This design puts apps on a hosted Clax (clax.dev by default) so that:

- a person can **publish** an app from the Clax app and **share** it with
  clax.dev's own sharing;
- anyone it is shared with can **use** it in the browser, or **get** it into
  their own Clax app;
- installed copies **notice and take updates** when the app is republished;
- a person's **other devices** pull their apps down and keep authoring them;
- **comments** on the public copy come back to the app and its agent.

The hosted side already does most of this for artifacts. It has device-code
sign-in (`clax remote add`), linked pushes with version history and refusal
on divergence (`clax push`), two-way comment threads, sharing with roles,
and forks (`docs/usage.md`, "Publishing to a public Clax";
`docs/contract.md`, "Publishing to a public Clax"). The app embeds the same
core (`crates/clax-embed` depends on `clax-server`, which holds
`public.rs` and `linked.rs`). The work is therefore mostly:

- bringing that machinery to the app's apps;
- reconciling it with the bundle model;
- building the app-side UX.

## 2. Goals, non-goals, anti-goals

**Goals**

1. Publish an app from the Clax app to a hosted Clax in one sheet. Publish
   later versions with one tap, or automatically per app.
2. Share it using clax.dev's existing permissions, from the app or from the
   web, as one setting.
3. Let anyone it is shared with run it on clax.dev with nothing installed,
   or get it into the Clax app as an app of their own.
4. Installed copies are offered updates without anyone sending a file. The
   update pipeline that tests on the recipient's data stays exactly as it is.
5. Every device signed in as the same clax.dev account is that app's author.
6. Every app shows one sync state, and nothing is overwritten silently in
   either direction.
7. Comments from clax.dev reach the app's comment tab and, once approved,
   its agent. The agent can answer them by publishing a fix.

**Non-goals**

- Moving or syncing a person's own data (documents) between their devices.
  Pulling brings apps only. Data movement is a later, separate design.
- Backups.
- A public directory or app store beyond the hosted Clax's existing "Public"
  general access and its public page.
- Changing how `.clax` files work when sent directly.

**Anti-goals** (failures even if everything technically works)

- A person's data reaching clax.dev without an explicit, per-publish
  choice.
- Anything public by default: a new publication visible to anyone but its
  owner, or commentable by anyone.
- An update replacing a person's changes, or an author's newer version,
  without asking.
- A stranger's text reaching an agent without the person's approval.
- A second, app-only permission model drifting from clax.dev's.

## 3. Owner decisions

These were settled by the owner on 2026-10-21 and are binding.

| # | Question | Decision |
|---|---|---|
| D1 | Should installed apps check the hosted Clax for updates? | **Yes**, for apps installed from or published to a hosted Clax. Updates are noticed (badge, inbox) and never applied without asking. The exception is when the person turns on automatic updates for that app, and even then the app stops to ask when an update adds a capability or a `net` host. This reverses the contract's "No server, directory or update check is involved" for linked apps only. |
| D2 | Who may publish an app's next version? | **The clax.dev account is authoritative for publication.** An app on clax.dev belongs to its account. Any device signed in as that account may publish its next version, and so may anyone the app's sharing gives Can edit. For linked apps this replaces trust-on-first-use by install key. Install keys still sign, and still decide trust for, `.clax` files sent directly. |
| D3 | Can data move between devices? | **No, not in this feature.** |
| D4 | Visibility on first publish | **Restricted** (only the owner). Sharing then uses the hosted Clax's existing sharing: people by GitHub login with Can view, Can comment or Can edit, and general access Restricted, Anyone with the link (view or comment) or Public. The app has no visibility model of its own. |
| D4a | Comments | **Off by default.** Each published app has a Comments switch, off at first publish. While it is off, nobody can comment, whatever their access, so Can comment acts as Can view. Once it is on, comments follow the sharing roles. |
| D5 | A modified installed copy meets an update | Offer **Update (replaces your changes)**, **Keep mine (stop updates)**, and a third option: **Bring v*N* into mine**. The app's agent merges the update into the person's version on a draft, and the result passes the same checks as any update. The third option shows only when the agent can run (an API key is set). |
| D6 | Which versions are publishable? | **Every version.** There is no separate release step. The person publishes the latest version, or picks one in Versions. |
| D7 | What does Can edit mean for an app? | **Editors may publish new versions**, from the web or from their Clax app. An editor's pull counts as the same app, not a stranger's copy. Their versions reach installed copies as updates, and recipients see the editor's name: "v8 by @junp · Habits by @akesling". |
| D8 | A version is published on the clax.dev web (by an editor or a hosted agent), so it carries no bundle | **Web edits diverge.** The push rule stays as it is today: the newest public version is the head, and a device's next publish that is not based on it is refused as moved. Each author device then resolves it as Diverged (5.10). Recipients are never offered a version that has no bundle. |

## 4. Constraints

- **Repository rules** (`AGENTS.md`):
  - crate layering (`scripts/check-crate-layers.py`);
  - no `unsafe` beyond `clax-embed`'s four exported functions;
  - no hard links;
  - tests fast and load-tolerant, with injected clocks and no sleeps;
  - migrations numbered contiguously in `crates/clax-store/src/migrations.rs`;
  - Bazel for builds.
- **The app's UI thread only draws.** Network calls, bundle building,
  hashing and polling run off it, as do core calls.
- **Reuse, don't fork.** The push, comment-link and sign-in code in
  `crates/clax-server/src/{public,linked}.rs` and
  `crates/clax-store/src/{remotes,linked,device}.rs` must serve both
  `clax push` from a daemon and the app. Bundle reading, writing, trust
  and update code stays in `apps/clax_app/lib/bundle/`.
- **The hosted side runs as the Cloudflare Worker** (`crates/clax-worker`,
  wasm32) and as a hosted-mode daemon. Every hosted change must work on
  both.
- **The bundle format is unchanged** (`format: 1`). Any new metadata
  travels beside the bundle, not inside its manifest, so older installs
  still read every bundle.
- **Copy** follows the app's rules: curly apostrophes (the app's
  `check_copy` fails on straight ones), and "Claude" for the agent.
- **Time to usable** matters (`web/perf/budget.json`). Publishing,
  updating and polling must not slow app launch or the say→usable path.
  Polling never blocks launch.

## 5. Design

### 5.1 Terms

- **Remote:** a hosted Clax the app is signed in to (clax.dev by default;
  any hosted Clax URL can be added). This is the same record as a daemon's
  remote (`clax_store::remotes::Remote`), held in the app's embedded home.
- **Linked app:** a local app whose `RemoteLink` names a public artifact on
  a remote. It becomes linked either because this device published it, or
  because it was installed or pulled from the remote.
- **Public copy:** that artifact on the remote. Its versions are the app's
  published versions.
- **Role here:** for a linked app, whether this device's signed-in account
  may publish (owner or editor) or only install (viewer or commenter).

### 5.2 Connecting (sign-in)

- **Where it starts:** Settings › Sharing › clax.dev › **Connect**. It
  also starts inline the first time the person chooses Publish to clax.dev
  or Get my apps.
- **Flow:** the app runs the existing device-code sign-in through the
  embedded core (`POST /api/remotes`, then `/poll`), with the machine name
  "Clax on <device name>".
  - The verification page opens in an in-app browser session
    (ASWebAuthenticationSession on Apple platforms, a Custom Tab on
    Android) or the default browser (macOS).
  - The app shows the user code with "Open clax.dev again" and Cancel. The
    code lasts 10 minutes.
- **Approval page:** says what the device may do: publish apps as the
  account, push their new versions, read their comments, and manage their
  sharing (see 5.6).
- **Token storage.** The sign-in yields the agent token that device sign-in
  already mints, labelled with the device's name. The core keeps it in its
  store (`clax_store::remotes`, `remote_token`), as a daemon does. Push and
  thread sync read it from there. *My call: reuse over moving it to
  platform secure storage. The embedded home already lives in the app's
  private support directory.*
- **Disconnect** revokes the token on the remote, as `clax remote remove`
  does. Public copies stay up, their links keep working, and local apps
  keep their link records as read-only until the person reconnects.

### 5.3 Publishing

**Entry points.** "Publish to clax.dev…" appears in the tile's long-press
menu above "Share app…", and on the inspector's Overview.

**The publish sheet** (first publish) shows:

- "to clax.dev as @login";
- Who can open it: **Restricted**, "Only you, until you share it", not a
  choice here;
- Include my data: off. It works as it does on Export, and is disabled
  past the seed caps;
- Publish new versions: Ask me (default) or Automatically;
- Comments: off;
- "Goes up / Stays here": the version, files and bytes going up; data,
  earlier versions, threads and the API key staying.

**What publishing does:**

1. The app builds the bundle exactly as Export does (`Exporter`), signed by
   this install's key. Lineage and bundle version follow 5.8.
2. The core pushes the app's selected version to the remote as the next
   artifact version. The push path is the existing one, in a **new mode**
   that sends one chosen local version rather than every unpushed version
   in order. Today's `POST /api/artifacts/<id>/push` always sends all of
   them, and its idempotency key includes the local version number; the new
   mode's key names the chosen version. The mode:
   - sends files, label and note as today;
   - creates the public artifact on first publish, with general access
     Restricted and comments off (5.6);
   - does **not** rewrite `/_blob/` asset references. Bundles carry no
     assets, so an app's files are pushed byte for byte, and the public
     version's content hash equals the bundle's `content_sha256`. An app
     whose files reference `/_blob/` addresses is refused at publish with
     "This app uses files Clax can't share yet". *My call.*
3. The bundle is uploaded beside that version (5.9) so that a pull
   downloads a byte-identical bundle. The remote stores it unread, subject
   only to a size cap (32 MiB, the reader's), and records the publishing
   account and the bundle's lineage and version alongside it.
   - **No bundle reader on the remote.** Recipients run the full existing
     read checks, and add one: the bundle's `content_sha256` must equal
     the public version's content hash, which the remote serves. *My call:
     a second bundle reader in Rust, with strict zip and strict Ed25519 on
     wasm32, would duplicate `apps/clax_app/lib/bundle/` for no extra
     safety, since every recipient verifies anyway.*
4. The result sheet shows "<title> is on clax.dev · only you so far", the
   address (`clax.dev/@login/<slug>`), **Share…** and **Open on clax.dev**.

**Rules:**

- Only versions the person publishes leave the device (D6). Unpublished
  agent edits stay local. This differs from `clax push`, which sends every
  version not yet pushed. The app pushes only the selected version, and
  intermediate local versions are skipped.
- **Later versions.** When a change lands on an app that is ahead (5.4),
  the app offers to publish it:
  - a toast on the app screen ("Claude made v5: <note> · Publish");
  - the ↑ badge;
  - an "↑ v5 here · v4 public" pill.

  Publish opens the version publish sheet. It shows the change note, the
  files changed, and anything that matters downstream: `migrate.js`
  present, new capabilities, new `net` hosts. It also has the switch for
  automatic publishing. Automatic publishing uses the existing `--follow`
  behaviour, with a refusal reported in the inbox.
- **Choosing a version.** Versions marks each version "here", "☁ public"
  or "☁ was public". Publishing an older version publishes its files as the
  next public version.
- **Offline.** The publish waits ("Waiting for a connection") and retries.
  A lost answer is safe to resend, as `clax push` already guarantees.
- **Unpublish**, on Overview with confirmation, deletes the public copy.
  Installed copies keep working, stop receiving updates, and show "No
  longer on clax.dev".

### 5.4 Sync state

A linked app is in exactly one of these states. It is shown as a tile
badge, as a pill on the app screen and on Overview.

| State | Condition | Badge | Actions |
|---|---|---|---|
| Only here | no link | none | Publish to clax.dev… |
| Live | the live local version's content hash and capabilities equal the public head's | ☁ | Copy link, Share…, Unpublish |
| Ahead | role here may publish; local versions newer than the last published or pulled one; public head unchanged since | ↑ | Publish v*N* |
| Update waiting | public head newer than what this device last took; the local app is unmodified (as "Updating" in the contract defines it) | ↓ (the only warm-coloured badge) | Update, Later |
| Kept mine | the person chose Keep mine on a modified copy | none (Overview shows it) | Check for updates, Publish as a new app |
| Diverged | both moved: local versions beyond the shared base, and a newer public head. A web-made head counts (D8) | ⑂ | Bring into mine / Combine, Take theirs, Publish as a new app |

For a recipient (viewer or commenter), "Ahead" never happens. Their local
changes make the copy modified, so a newer head is offered as a
modified-copy update (D5) rather than as "Diverged".

### 5.5 Getting an app

**On clax.dev,** the artifact page of a published app gets a header with:

- the icon, title and version;
- "by @owner" (and "v*N* by @editor" when an editor published the head);
- the install count;
- **Fork**;
- **Get in Clax**.

The app runs live on the page. Without seed data it starts empty. What a
browser visitor enters is kept by the page's normal `db` rules on clax.dev,
not on any device.

**Get in Clax** is an `https` link (`/<artifact path>/get`):

- **Phones:** a universal link (iOS) or verified app link (Android) opens
  the Clax app.
- **Desktop browsers:** a sheet shows a QR code of the same link, **Open in
  Clax for Mac** (a `clax:` URL scheme or universal link handled by the
  macOS app), and **Download .clax**.
- **Without the app:** the link's web fallback offers the stores, then the
  page.
- **Other hosted Clax instances.** Universal links and verified app links
  work only for domains in the app's entitlements, so they cover clax.dev
  alone. Any other remote offers **Download .clax** and a `clax:` URL
  (`clax:get?url=<artifact URL>`) that the app handles on every platform.
  Its **Get my apps** also takes a pasted artifact link. *My call.*

**In the app,** the link fetches the bundle (5.9) and runs the existing
read checks and install sheet. The changes are:

- **Provenance line:** "From @owner on clax.dev", replacing the fingerprint
  line. The fingerprint stays under Details.
- **Trust answer:** a new `linked` answer (5.8) replaces `install`,
  `foreign` and the others when the bundle came from a remote and the
  device is signed in to that remote, or when the remote vouches for the
  bundle (5.9).
- **Link record:** the installed app is linked to the public copy, which
  turns on update checks and threads.
  - The link is created **at install or pull time**, with its base set to
    the version taken, so a pulled copy that has never pushed still has a
    base for the moved check.
  - Without a signed-in remote, the link holds only the artifact's URL.
    Update checks then use the anonymous head route (5.7), and threads are
    off until the person connects.

**Get my apps** sits on the empty home screen and in the launcher's ⋯ menu
beside Import app…. It lists, for the signed-in account:

- the apps it owns on the remote;
- the apps shared with it at Can edit;
- the apps shared with it at view or comment.

The person picks which to bring down. Owned and editable apps arrive as
authored copies, whose role here may publish. Apps come without data (D3),
and the list says so.

### 5.6 Sharing

- **Share…** on the result sheet, on Overview and in the long-press menu
  opens a native sheet with the same model as the web Share dialog
  (`web/shell/src/ui/share-model.ts`):
  - an add field (a GitHub login or a known name) with Can view, Can
    comment or Can edit;
  - People with access;
  - General access (Restricted, Anyone with the link at view or comment,
    Public where the remote has a public gallery);
  - **Copy link** and the platform share sheet, enabled once anyone but the
    owner can open the app;
  - the **Comments** switch (D4a), above the roles.
- Changes made here and on the web are one setting, stored by the
  remote's existing sharing routes (`crates/clax-server/src/hosted.rs`,
  `put_share`, `delete_share`, general access).
- **Remote change, token access:** those routes are browser-only today
  (`browser_only` in `hosted.rs` refuses any `Authorization` header), and
  device sign-in mints an ordinary agent token. The remote must let a
  token minted by the Clax app's sign-in manage sharing for artifacts its
  account owns or made.
  - This takes a marker on that token ("person's device"), set when the
    sign-in names itself as the Clax app (`client_kind: "clax-app"` on
    `POST /api/hosted/device/code`; `grants.person_device`).
  - *As built:* the marker is honoured in the agent verification layer,
    not in `browser_only`. `verify_bearer` (`crates/clax-http/src/agent.rs`)
    hands a device's token on the routes of `PERSON_DEVICE_ROUTES` (the
    sharing routes, the Comments switch and the apps library) a
    `PersonDevice` naming its person, while the token is live and its
    person a member, and the handlers' `browser_or_device`
    (`crates/clax-server/src/hosted.rs`) acts for that person as for
    their browser session; with no `PersonDevice` they fall back to
    `browser_only`.
  - Every other agent token stays refused (403 `forbidden`).
  - People on the remote lists such tokens apart from agents.
- **Remote change, Comments switch:** a per-artifact `comments` flag, on
  by default for every existing and web-created artifact and off for
  artifacts created by an app publish. While it is off, the remote refuses
  comment creation (thread or reply) by anyone, owner included. Existing
  threads stay readable. The web Share dialog shows the same switch.
- **Who may share:** as on the remote today, only the owner and the maker
  share or delete. Editors publish but do not share.

### 5.7 Updates

- **When the app checks:**
  - when the app comes to the foreground, at most once per 15 minutes per
    remote;
  - when a linked app opens;
  - in the platform's background refresh where available (Android
    WorkManager, iOS BGAppRefresh), opportunistically.

  Checks never block launch or rendering.
- **How it checks.** When signed in, the app uses the remote's existing
  linked-artifact polling. A new read route, `GET <artifact>/head`,
  answers anyone who may open the artifact, with or without a token. It
  returns:
  - the head version;
  - its content hash;
  - its publishing account;
  - the newest version that carries a bundle, with that bundle's lineage
    version and URL.

  Apps installed from an "Anyone with the link" or Public page therefore
  get updates without connecting. A Restricted app needs the person signed
  in, as opening it already does. *My call: update access equals view
  access.*
- **What counts as an update.** For recipients, only versions that carry a
  bundle (D8).
- **What a newer head produces:**
  - the ↓ badge;
  - an inbox item ("<title> v*N* is ready · from @who", with Update);
  - nothing applied.

  With automatic updates on for that app (a switch on the update sheet and
  Overview, off by default), the app runs the update itself. It still stops
  and asks when the update adds a capability or a `net` host.
- **Running an update:** exactly today's `Updater` pipeline (draft,
  `migrate.js`, inspect, tests, snapshot, publish, migrate, rollback), fed
  the bundle downloaded from the remote. The prompt names the publisher
  (D7).
- **Modified copy:** the three choices of D5. **Bring v*N* into mine**
  starts an agent task on a draft with:
  - the person's live version;
  - the incoming bundle's files;
  - the common base: the release this copy last took, kept by the lineage
    store.

  The task's instruction is to keep the person's changes and take the
  update's. Its result then runs the pipeline's checks (inspect, tests,
  `migrate.js` on the draft's copy of the data). It is offered as "Your
  version with v*N*", with Publish to this app and Discard. Afterwards the
  app is treated as unmodified-at-*N* plus the person's changes; that is,
  it records release *N* as taken.
- **Failure:** today's "v*N* didn't pass its checks on your data:
  <reason>. Your app is unchanged.", with Details. New: when the person may
  comment on the public copy (comments on, and Can comment or above),
  **Tell @owner** opens a composer prefilled with the failure reason and
  the version. It sends no data and is shown before sending, and posts a
  comment on the public copy.

### 5.8 Trust, lineage and versions (D2, D7)

- **New trust answer `linked`,** checked before the existing table: the
  bundle came from remote *R* for public artifact *A*, and a local app is
  linked to *R*/*A*. Then:
  - a higher version is an `update` whatever key signed it;
  - the same version with the same content hash is `current`;
  - a lower version is `older`.

  The signature is still verified for integrity, and the signer's
  fingerprint is recorded with the release. The first-use key rule is
  skipped. Who may publish is the remote's decision, made when it accepts
  the push.
- **First install** from a remote creates the link and a lineage entry
  whose upstream is *R*/*A* rather than a key.
- **Lineage:**
  - One lineage per public copy. First publish uses the app's existing
    lineage when it has been exported before, and otherwise mints one as
    first export does.
  - A device publishing an app it pulled uses the public copy's lineage.
- **Two counters:**
  - **Artifact versions** on the remote. They decide "moved": a publish
    must be based on the current head artifact version, else it is
    refused (5.10, D8).
  - **Bundle `version`**, the lineage's release counter. Each bundle
    pushed to a public copy must be greater than the last bundle the copy
    holds. On first publish it continues from the app's last export
    (`exportVersion`), so recipients of earlier files see the cloud bundle
    as a newer version of the same lineage. The number may skip but never
    goes back. *My call.*
- **Editors' pushes:** the remote accepts a push from a token whose person
  holds the `update` right on the artifact (owner, or Can edit through
  `clax_store::shares`). Today push has only run with the owner's token, so
  this is a remote change.
  - `forked_from` on a fork names the original lineage, version and the
    original head's signer key, as today.
- **Editors:** a bundle's `author` stays the signing install's key and
  name. The remote records, per published version, the publishing account
  (owner or editor). Install and update sheets show "v*N* by @editor ·
  <title> by @owner" when they differ.
- **Files sent directly** (Share app…) are unchanged. Share app on a
  linked app shares the last bundle this device published or took, byte
  for byte.

### 5.9 The bundle on the remote

- **Storage:** each public version published from the app carries one
  stored bundle, never one of the version's served files, counted with
  its files against the version's size limit. *As built:* a row of the
  `app_bundles` table (artifact, version, lineage, bundle version, the
  files' content hash, the bundle's own hash and size, and the publishing
  account; `crates/clax-store/src/app_bundles.rs`), and its bytes in the
  content store as the artifact's asset `bundle-<sha256>.clax`.
- **Read route:** the remote serves it at a stable URL for the head and
  per version, to anyone who can open the artifact, with the publishing
  account and the version number in response headers or a small JSON
  sidecar.
- **Versions published from the web:** an editor republishing on clax.dev,
  or a hosted agent, produces no bundle, and the remote builds none.
  - Such a version becomes the head, and author devices' next publishes are
    refused as moved (D8, 5.10).
  - Recipients are not offered it. They next see an update when an author
    device publishes a version with a bundle, which will contain the web
    changes if that device combined or took them.

### 5.10 Divergence

- **Refusal:** a publish whose base is not the public head is refused,
  using the existing "moved" refusal in `public::push`. The app shows the
  Diverged state with:
  - **Combine** (or **Bring into mine** for a non-author): an agent merge
    on a draft as in 5.7, then publish as head + 1;
  - **Take theirs**: update to the head, discarding local versions after
    the base; they stay in Versions. When the head has a bundle, this runs
    the update pipeline. When it is web-made (D8), the app reads the head's
    files through the remote's existing read routes and publishes them as
    a local version, running the same draft checks first;
  - **Publish as a new app**: a new public artifact and lineage, with
    `forked_from` set, and this device's local app relinked to it.
- **Merge record:** a combined version's note names both sources ("from
  v8 on Alex's Mac and v8 here"). Whether the remote records two parents
  is left to the builder.

### 5.11 Comments

- **In the app:** a linked app's threads from the remote appear in the
  app's comment tab tagged with the source ("clax.dev") and the author's
  name there. This uses the existing two-way link (`linked.rs`) and the
  inbox.
- **Withheld text:** comment text from people the person has not approved
  stays withheld from the app's agent under the existing approval rules.
  "Send to Claude" on such threads runs the approval card.
- **Publish and reply:** the agent's reply to a remote thread waits for the
  fix to be public when the thread is addressed by a local version.
  **Publish v*N* and reply** publishes the version, lists the thread as
  addressed, and posts the reply. A reply with no new version posts at
  once.
- **Threads started in the app** on a linked app go to the remote as the
  person, as `clax push` does (two-way by default). Comments-off on the
  remote refuses them, and the app hides the composer for linked apps with
  comments off.

### 5.12 Forks

- **Fork on the web:** Fork on clax.dev, then Get in Clax, gives a fork
  linked to the forker's own public copy. Its lineage is new, and
  `forked_from` names the original.
- **Fork in the app:** "Share my version as a new app" on a modified
  installed app becomes **Publish fork** when connected. It opens the
  publish sheet (Restricted, comments off) with the name prefilled
  "<title> (yours)".
- **On the original's page:** "forked from @owner/<slug>" on forks, and a
  fork count on originals.

### 5.13 Data

- Publishing never sends documents unless Include my data is on for that
  publish. That choice is never remembered, and it follows the existing
  seed rules.
- Pulls never bring documents.
- Updates keep the recipient's documents, as today.

## 6. Documentation to change

- `docs/contract.md`, "Sharing apps":
  - linked apps check for updates (the D1 exception);
  - the core now has routes involved in sharing apps (the selected-version
    push mode, bundle storage, the head route), which ends "the core adds
    no route for it";
  - the `linked` trust answer;
  - lineage and version numbering for linked apps;
  - the stored bundle routes;
  - "Tell @owner".
- `docs/contract.md`, "Publishing to a public Clax": the app as a client;
  the device token kind; sharing routes open to it; the per-artifact
  comments flag; editor attribution per version.
- `docs/usage.md`: the Clax app's publish/share/get flows, and clax.dev's
  Get in Clax.

## 7. Security and privacy

- **Data:** none leaves without the per-publish switch. The remote token
  lives in the embedded core's store, inside the app's private support
  directory (5.2).
- **Token scope:** the device token may publish, push, read comments and
  manage sharing only for the account's own and editable artifacts. It
  cannot reach People or approvals, and is listed and revocable on the
  remote.
- **Pushes:** the remote accepts a version only from the owner or an
  editor, and caps bundle size.
- **Downloads:** downloaded bundles pass every existing read check before
  any write, including the 32 MiB cap and the refusal of `db` `read` or
  `share`. They must also match the public version's content hash.
- **Comments:** withheld-until-approved still governs agent access, and
  comments are off by default.

## 8. Testing

- **Core (Rust):** app-style pushes of a selected version; the moved
  refusal; the comments flag; the device token's rights, including refusal
  of People and approvals; stored-bundle verification and serving;
  editor-attributed versions. Run against `clax-testkit`'s server, and on
  the Worker through its existing e2e harness (`crates/clax-worker`).
- **App (Dart):**
  - the trust table with `linked` rows;
  - the sync-state function as a pure function over (local versions,
    release record, public head, modified flag, kept flag), with
    property-style tests over sequences of publish, update, edit and
    remote publish;
  - the publish and update sheets;
  - the Bring-into-mine flow with a fake agent.
- **End to end:** an app publishes, a second app home pulls it, the first
  republishes, and the second is offered and takes the update. Run against
  a local hosted daemon on a scratch `CLAX_HOME` and free port, never the
  owner's.
- **Constraints:** no sleeps; polling intervals use injected clocks.

## 9. Left to the builder

- The stored bundle's exact path or asset form, and its route shapes.
- How the "person's device" marker is stored on the remote's tokens, and
  the migration it needs. *As built:* migration 48
  (`crates/clax-store/src/migrations.rs`, `APP_PUSH_MIGRATION`) adds
  `grants.person_device` and `device_codes.client_kind` with the rest of
  this feature's schema: `app_bundles`, `app_installs` and
  `artifacts.comments`.
- Background-check scheduling details per platform within 5.7's bounds.
- Whether a combined version records two parents.
- The visual design of badges and sheets within the app's tokens. The
  storyboard (ahead blue ↑, update warm ↓, live green ☁, diverged amber ⑂)
  is a starting point.
- The order of work. A sensible order:
  1. remote changes (token, comments flag, bundle storage, attribution);
  2. app connect and publish;
  3. get and pull;
  4. updates;
  5. comments;
  6. divergence and forks.

## 10. Rollout

- **The hosted Clax ships before the app.** A public Clax from before
  this feature refuses the app's sign-in (`client_kind` is a field its
  device sign-in does not know), ignores an app publish's `comments` and
  `forked_from`, and lacks the bundle, head, apps and comments routes.
  Deploy clax.dev (and tell self-hosted instances to upgrade) before any
  app build that connects to it is released.
- **What the app shows against an older one.** The core answers 502
  `remote_too_old`, naming the remote's origin, for each of those: the
  sign-in refused for `client_kind`, an app publish whose create answers
  no Comments switch (the copy just made there is deleted), and a
  forwarded route answered 404 or 403 `browser_required` by a remote that
  also lacks `GET /api/hosted/apps` (`docs/contract.md`, "Publishing to
  a public Clax").
