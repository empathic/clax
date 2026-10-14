# Clax app: app bundles and updates

Date: 2026-10-13. Branch `flutter-app`. Status: prototype. The owner's
decisions (§1) are binding; everything else is this design's. Paths not
taken are qualifier `alternative` records on this file.

Extends `2026-10-12-flutter-app-design.md` ("base §n"),
`2026-10-12-flutter-app-round2-design.md` ("R§n") and
`2026-10-12-flutter-real-apps-design.md` ("C<n>").

## 1. Owner decisions (binding)

- **Bundle.** One file `<name>.clax` (a zip): a manifest (format version,
  lineage ID, version number, title, icon, declared capabilities, kit
  version, content hash, author public key, signature), the app's files,
  optional seed data. Seed data is the sender's choice at export, default
  none. Never included: owner grants, comment threads, the API key, other
  apps' data.
- **Export.** Phone: "Share app" (launcher menu and inspector) through the
  share sheet. Mac: "Export…" to a file.
- **Import.** `.clax` files open in Clax (Android intent filter, macOS
  document type) and show an install sheet: icon, title, from whom (the
  author key's short fingerprint and the manifest's name), declared
  capabilities. Installing and using an app needs no API key; changing it
  or `sample` does.
- **Identity.** A per-install Ed25519 key in secure storage, made at the
  first export. The lineage ID is fixed at an app's first export. Same
  lineage and same key: offer **Update**. An existing lineage under another
  key installs only as a separate app.
- **Re-share** is allowed and keeps the original lineage and signature.
- **Updates are full bundles.** An update lands as a draft; its tests and
  page check run against a copy of the recipient's data; only on a pass
  does it become the live app's next version (the previous ones kept,
  rollback by version restore). On failure: keep current. Consent only for
  added capabilities. Optional `migrate.js`, run in the draft before the
  tests.
- **Local modifications: replace or fork.** "Update (replaces your
  changes)" or "Keep mine (stop updates for this app)".
- **The agent is told apps may be shared:** no personal data hard-coded in
  files; data in `db`.

Ruled by the owner on 2026-10-13, on this design's own proposals:

- **Cross-app reads** are stripped at export, and a bundle carrying
  `db.read` or `db.share` is refused (§4.4).
- **The live migration is snapshotted:** the app's documents are
  snapshotted before `migrate.js` runs on live data; if it fails part way,
  both the version and the documents roll back (§8.2, §8.4).
- **Downgrades are refused** (§6.4 `older`).
- **Consent:** capabilities are listed at install and asked on first use
  (§8.2).

## 2. Goals and non-goals

Goals:

- Send an app to another Clax install as one file, through any channel
  that carries files (messages, mail, AirDrop, a USB stick).
- The recipient installs it in two taps and uses it with no API key.
- The author ships later versions; the recipient's data survives them, and
  a version that breaks on the recipient's data never goes live.
- A bundle cannot take over an app it did not come from.

Non-goals (this round):

- A store, a directory, or any server: bundles travel as files only.
- Automatic update checks. An update is a bundle the recipient opens.
- One identity across a person's devices, key backup or rotation (each
  install is its own author).
- Patches, delta bundles, merging the recipient's changes into an update.
- Cross-app reads in shared apps (§4.4), assets uploaded through the
  `assets` capability, the build plan (`PlanStore`), version history.
- Any daemon change (§7) beyond naming the migration view's frame
  (§8.3), and any migration (§11).

## 3. User flows

### 3.1 Export (author)

1. Launcher tile, long-press → **Share app** (Mac: **Export…**); or the
   inspector's Overview → the same button.
2. The export sheet shows the icon, the title, "v<N>" (the version this
   export will carry, §5.2), what goes ("<k> files, <size>") and a switch
   **Include my data** (off), with "<d> documents" beside it, disabled with
   "Too much data to include" past the seed caps (§4.3). A **Your name**
   field shows when Settings has none (optional; kept in Settings).
3. Share / Export: the bundle is built (§5), then the phone opens the
   system share sheet with `<slug>.clax` (MIME
   `application/vnd.empathic.clax+zip`); the Mac opens a save panel
   (`file_picker` `saveFile`, default name `<slug>.clax`).

`<slug>`: the title lower-cased, runs of characters outside `a-z0-9`
replaced by one `-`, trimmed of `-`, cut to 40, `app` when empty.

### 3.2 Import → install (recipient)

1. The person opens a `.clax` file (Files, a message, Finder, or the
   launcher's **Import app…** item on both platforms, `file_picker`).
2. The bundle is read and verified (§6.2). A refusal shows one sentence
   ("This file is damaged or was changed after it was signed") and Close.
3. The trust decision (§6.4) picks the sheet. For a new lineage, the
   **install sheet**: icon, title, "v<N>", "From <name> · <fingerprint>"
   (or "From <fingerprint>" without a name; "From you" for this install's
   key), "<name> is the sender's own description" in small type, what it
   can ask for (each declared capability in words, the network hosts by
   name), "Comes with <d> documents" when seeded. **Install** / Cancel.
4. Install (§8.1); then the app opens.

### 3.3 Update

The bundle's lineage is installed here with the same key, at a lower
version.

- **Unmodified copy** (§6.5): "Update <title> to v<N>? You have v<M>." plus
  "This version also asks to: …" when capabilities were added. **Update** /
  Not now.
- **Modified copy:** "You changed <title> since v<M>." **Update (replaces
  your changes)** / **Keep mine (stop updates for this app)**. Keep mine
  marks the lineage `updates: stopped` (§5.3); a later bundle of it shows
  this same choice, with "You kept your changes on <date>".
- Update runs the check (§8.2) under a progress sheet: "Checking v<N>
  against your data…", with the step (data, page, tests). On a pass:
  "Updated to v<N>", **Open**. Nothing else changes for the person: their
  documents stay, the previous versions stay in the Versions tab.

### 3.4 Failure

The check failed: "v<N> didn't pass its checks on your data: <reason>.
Your app is unchanged." with **Keep current** (closes the sheet) and
**Details** (the failing step's messages: migration error, uncaught page
errors, failed test names and errors). Nothing was published; the draft is
deleted. The same bundle can be opened again later.

### 3.5 Re-share and fork

**Share app** on an installed app:

- **Unmodified copy:** the kept original bundle (§5.3) goes as received,
  byte for byte: the author's signature, the author's seed choice. The
  sheet says "Shares <author>'s v<N> as you received it" and has no data
  switch, so the recipient's documents never travel in someone else's
  bundle.
- **Modified copy:** two choices. **Share <author>'s v<N>** sends the kept
  original as above. **Share my version as a new app** forks: a new
  lineage under this install's key, version 1, `forked_from` naming the
  original (§4.2), the data switch as in §3.1. From then on this install is
  the author of the fork's lineage, and the local app no longer takes the
  original's updates: a later bundle of the original lineage and key shows
  "You made <title> your own. Install <author>'s v<N> as a separate app?"
  **Install separately** / Cancel.

Updates therefore come only from the key that signed the lineage's first
bundle this install saw; a re-sharer cannot sign for the author.

## 4. Bundle format

### 4.1 Layout

A zip archive (PKWARE APPNOTE 6.3, no zip64, no encryption), entries in
this order, each `stored` or `deflate`d, every timestamp 1980-01-01 00:00,
no directory entries, no extra fields, no comments:

| Entry | Content |
|---|---|
| `manifest.json` | §4.2, UTF-8, no BOM. |
| `manifest.sig` | The signature (§4.5): 86 base64url characters, no padding, no newline. |
| `files/<path>` | Each file of the version, `index.html` included, in byte order of `<path>`. |
| `seed.jsonl` | Present only when the manifest's `seed` is not null (§4.3). |

The writer is deterministic: the same manifest and files give the same
bytes. No other entry is allowed.

The kit (C1) is not served by the shell: `add_kit` copies its files into
the app under `kit/` (`lib/agent/tools/kit_tool.dart`), so they travel in
`files/kit/…` like any file. A bundle therefore runs the kit it was made
with, whatever kit the recipient's Clax ships, and the kit imposes no
compatibility rule; the manifest's `kit` only records which one it is. The
runtime contract (the shell, the bridge, `window.claude.use`) is the
recipient's; a capability the recipient's daemon does not know fails the
install with `invalid_capabilities` (§8.1).

### 4.2 `manifest.json`

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["format", "lineage", "version", "title", "icon",
               "capabilities", "kit", "files", "content_sha256", "seed",
               "author", "forked_from"],
  "properties": {
    "format": {"const": 1},
    "lineage": {"type": "string", "pattern": "^l_[a-z2-7]{26}$"},
    "version": {"type": "integer", "minimum": 1, "maximum": 2147483647},
    "title": {"type": "string", "minLength": 1, "maxLength": 200},
    "icon": {
      "type": "object", "additionalProperties": false,
      "required": ["svg", "word"],
      "properties": {
        "svg": {"anyOf": [{"const": "icon.svg"}, {"type": "null"}]},
        "word": {"anyOf": [{"type": "string", "pattern": "^[a-z][a-z0-9-]{0,31}$"},
                           {"type": "null"}]}
      }
    },
    "capabilities": {"type": "object"},
    "kit": {"anyOf": [{"type": "string", "pattern": "^sha256:[0-9a-f]{64}$"},
                      {"type": "null"}]},
    "files": {
      "type": "object", "minProperties": 1, "maxProperties": 512,
      "propertyNames": {"pattern": "^(?!.*(//|/$|(^|/)\\.{1,2}(/|$)))[A-Za-z0-9_][A-Za-z0-9_.\\-/]{0,199}$"},
      "additionalProperties": {
        "type": "object", "additionalProperties": false,
        "required": ["sha256", "size", "type"],
        "properties": {
          "sha256": {"type": "string", "pattern": "^[0-9a-f]{64}$"},
          "size": {"type": "integer", "minimum": 0, "maximum": 16777216},
          "type": {"type": "string", "maxLength": 100}
        }
      }
    },
    "content_sha256": {"type": "string", "pattern": "^sha256:[0-9a-f]{64}$"},
    "seed": {
      "anyOf": [{"type": "null"}, {
        "type": "object", "additionalProperties": false,
        "required": ["sha256", "size", "docs"],
        "properties": {
          "sha256": {"type": "string", "pattern": "^[0-9a-f]{64}$"},
          "size": {"type": "integer", "minimum": 1, "maximum": 8388608},
          "docs": {"type": "integer", "minimum": 1, "maximum": 2000}
        }
      }]
    },
    "author": {
      "type": "object", "additionalProperties": false,
      "required": ["key", "name"],
      "properties": {
        "key": {"type": "string", "pattern": "^[A-Za-z0-9_-]{43}$"},
        "name": {"anyOf": [{"type": "string", "minLength": 1, "maxLength": 64},
                           {"type": "null"}]}
      }
    },
    "forked_from": {
      "anyOf": [{"type": "null"}, {
        "type": "object", "additionalProperties": false,
        "required": ["lineage", "version", "key"],
        "properties": {
          "lineage": {"type": "string", "pattern": "^l_[a-z2-7]{26}$"},
          "version": {"type": "integer", "minimum": 1},
          "key": {"type": "string", "pattern": "^[A-Za-z0-9_-]{43}$"}
        }
      }]
    }
  }
}
```

Further rules the schema cannot say:

- `files` holds `index.html`; when `icon.svg` is not null, `files` holds
  `icon.svg`. Paths are unique ignoring ASCII case. Total size of `files` at most 64 MiB (the daemon's
  `body_too_large`).
- `capabilities` is the version's full declaration, passed to the daemon
  unchanged at install. It never holds `db.read` or `db.share` (§4.4).
- `lineage`: `l_` and 16 random bytes in lower-case RFC 4648 base32
  without padding.
- `author.key`: the raw 32-byte Ed25519 public key, base64url, no padding.
- `kit`: the content hash (§4.5's formula) of the files under `kit/`
  alone, or null when there are none. Informational.
- The writer emits keys in the order of the schema's `required` list, two
  spaces of indentation, `\n` line ends, a final `\n`. Readers do not
  depend on it: the signature covers the bytes (§4.5).

### 4.3 `seed.jsonl`

One line per document, sorted by `path` in byte order, each
`{"path": "<collection>/<doc>[/<sub>/<doc>…]", "data": <object>}` with no
other keys, `\n` after each line. The app's own documents at the owner's
level, read as the daemon's draft copy reads them: `data/users/<id>/`
subtrees left out. At most 2,000 documents and 8 MiB; a path under
`data/users/` refuses the bundle.

### 4.4 Cross-app reads are not shared

`db.read` and `db.share` name the author's artifact IDs, which mean
nothing on another install. Export removes both keys from `db` (and the
`db` object keeps its other members); a bundle that carries either is
refused. A page that called `db.artifact(id)` gets `not_declared` on the
recipient's install, as the contract already says for an undeclared read.
The author's own app keeps its declaration; only the bundle drops it.

### 4.5 Hashes and the signature

- **File hash:** lower-case hex SHA-256 of the file's bytes.
- **Content hash** (`content_sha256`): `sha256:` and the hex SHA-256 of
  one line `<path>\0<sha256 hex>\0<size>\n` per file in byte order of
  path. This is the daemon's version `content_sha256` (`docs/contract.md`,
  "Versions, seen marks and attention"), so the version an install
  publishes must report exactly this value (§8.1).
- **Seed hash:** hex SHA-256 of `seed.jsonl`'s bytes.
- **Signing input:** the 14 bytes `clax-bundle/1\n` followed by the exact
  bytes of the `manifest.json` entry. The manifest holds every other hash,
  so the signature covers the files and the seed.
- **Signature:** Ed25519 (RFC 8032, pure, no context) by `author.key`'s
  private key over the signing input, verified strictly (canonical `S`,
  canonical point encodings).

The signature sits in its own entry rather than inside the manifest so the
signed bytes are the stored bytes, with no JSON canonical form to agree on.

## 5. Identity, lineages and versions

### 5.1 The install's key

`BundleKey` (`lib/platform/bundle_key.dart`): a 32-byte Ed25519 seed in
`flutter_secure_storage` under `bundle_signing_seed` (base64url), with the
same macOS options as the API key (`FlutterSecureKey.macOsOptions`). Made
from `Random.secure()` at the first export; never logged, never given to
the daemon or the WebView, never in a bundle. Its **fingerprint**: the
first 10 bytes of SHA-256 of the public key in upper-case RFC 4648 base32,
as four groups of four (`7KQ2-M9XD-4FTA-QW3B`). Settings shows it under
"Sharing", with the **Your name** field. Uninstalling Clax loses the key
(§9, T9).

### 5.2 Version numbers (author side)

An app's **release state** is its live version's content hash, title and
capabilities (as exported, §4.4). The first export of an app mints its
lineage and carries version 1. A later export carries the last exported
version again when the release state is unchanged, else that plus one. The
data switch does not change the version: seed data is not part of an
update (§8.2).

### 5.3 The lineage store

`LineageStore` (`lib/bundle/lineage_store.dart`): one JSON file
`<support>/lineages.json`, written to a temporary file and renamed (as
`FileDeviceGrants`), keyed by artifact ID:

```json
{
  "<aid>": {
    "lineage": "l_…",
    "role": "author",
    "key": "<author key, base64url>",
    "name": "Alex",
    "version": 3,
    "released": ["sha256:…"],
    "capabilities": {},
    "title": "Plant log",
    "updates": "on",
    "stopped_at": null,
    "upstream": null,
    "from_bundle": false
  }
}
```

- `role`: `author` (this install signs it) or `installed`.
- `released`: every content hash the lineage's author released that this
  app has carried here (the author: each exported state; the installed:
  each installed or updated version).
- `capabilities`, `title`: of the latest release here.
- `updates`: `on` or `stopped` (Keep mine), `stopped_at` its time.
- `upstream`: `{lineage, key}` of the original, on a fork (§3.5).
- `from_bundle`: true when this app's code came from a bundle, kept true
  through a fork (§9, T2).
- The original bundle of an installed app is kept, byte for byte, at
  `<support>/bundles/<aid>.clax` (replaced on each update), for re-share.
- An `artifact_deleted` event for `<aid>` removes its entry and its kept
  bundle.

### 5.4 Modified or not

An installed app is **unmodified** when its live version's
`content_sha256` is in `released` and its capabilities equal the stored
`capabilities`; else **modified**. A restore of an earlier release is
therefore still unmodified; any agent change, or a restore of one, is
modified.

## 6. Reading and trusting a bundle

### 6.1 Caps

| What | Cap |
|---|---|
| The `.clax` file | 32 MiB |
| Entries | 515 (512 files and three) |
| `manifest.json` | 256 KiB |
| One file, inflated | 16 MiB, and exactly its manifest `size` |
| All files, inflated | 64 MiB |
| `seed.jsonl` | 8 MiB, 2,000 lines |
| Compression | `stored` (0) or `deflate` (8) |

### 6.2 Verification, in order

`BundleReader.read(bytes)` returns a `Bundle` or throws `BundleError(code,
message)`. It never writes an entry to disk; everything stays in memory
until it is sent to the daemon.

1. File size cap. Parse the central directory; refuse zip64, encryption
   (flag bit 0), a method other than 0 or 8, a name that is not one of
   §4.1's, a duplicate name, an entry count over the cap, a local header
   whose name differs from the central one.
2. Inflate `manifest.json` with a counter (stop past 256 KiB), parse it
   against §4.2 and its further rules. `format` greater than 1:
   `too_new` ("Made by a newer Clax: update Clax to open it").
3. Verify `manifest.sig` against the signing input with `author.key`.
4. Each `files/<path>` entry: present for every manifest path and no
   other; inflated with a counter that stops at the manifest `size` plus
   one byte, whatever the zip header says; size and SHA-256 equal the
   manifest's. Recompute the content hash and compare.
5. `seed.jsonl`: present exactly when `seed` is not null; size, hash,
   line count; each line parses with only `path` and `data`, `data` an
   object, the path valid for the daemon's documents and not under
   `data/users/`, paths unique.
6. `icon.svg`, when named, passes `cleanIcon` (`lib/agent/icon_svg.dart`);
   one that cannot be drawn is shown as the tile face, not refused.

Error codes: `too_large`, `not_a_bundle`, `too_new`, `bad_manifest`,
`bad_signature`, `bad_file`, `bad_seed`, `not_shareable` (`db.read` or
`db.share`).

### 6.3 Paths

Bundle paths match the manifest pattern (§4.2): relative, `/`-separated,
no empty, `.` or `..` segment, no backslash, no control character, at most
200 bytes. They are then the daemon's paths, checked again by its own path
validation when the files are published. Nothing touches the file system,
so a traversal has nothing to traverse.

### 6.4 The trust decision

`decide(bundle, store, ownKey)` (`lib/bundle/trust.dart`, pure) answers
one of:

| Case | Answer | Sheet |
|---|---|---|
| `author.key` is this install's key and the lineage is a local `author` app | `own` | "This is your app <title>." **Open**. |
| No local app has the lineage, none has it as `upstream` | `install` | Install sheet (§3.2). |
| A local `installed` app has the lineage with the same key, bundle version greater | `update` (with `modified` from §5.4, `stopped` from the store) | §3.3. |
| Same lineage and key, same version, same content hash, title and capabilities | `current` | "You already have v<N>." **Open**. |
| Same lineage and key, same version, anything of those differs | `conflict` | "This file claims to be v<N> of <title> but differs from the v<N> you have. It was not installed." Close. |
| Same lineage and key, lower version | `older` | "This is v<N>; you have v<M>. Older versions are not installed: use Versions to go back." Close. |
| The lineage is a local app's under another key | `foreign` | Install sheet with "<title> is already installed from someone else. This installs it as a separate app." **Install separately**. |
| A local fork has the lineage and key as `upstream` | `forked` | §3.5's "Install separately". |

A separate install (`foreign`, `forked`) makes a new app with its own
lineage entry under the bundle's key, so its own later bundles update it.
Two local apps may then share a lineage ID under different keys; the
store is keyed by artifact ID, and `decide` matches on lineage and key
together.

### 6.5 What the decision rests on

Trust is first use per lineage: the key of the first bundle of a lineage
this install installs is the only key that may update that app. The
author's name is the sender's own claim, always shown next to the
fingerprint. Deleting the app forgets the pin.

## 7. Where the work happens: Flutter only

No daemon route is added. Everything a bundle needs exists:

- **Reading an app:** `GET /api/artifacts/<aid>` (versions with each
  file's `sha256`, size and type, and `content_sha256`) and `GET
  /api/artifacts/<aid>/versions/<n>/files/<path>` (raw bytes); documents
  by `docs:collections` and the paged docs list.
- **Writing an app:** `POST /api/artifacts` and `POST /api/drafts` take
  publish-shaped file entries, `encoding: "base64"` included, with
  `content_type`; `docs:batch` writes documents; `versions:restore` rolls
  back.
- **Checking an update:** drafts copy the app's documents (R§5.1); the
  inspector loads, inspects and tests a draft (R§5.2, C2).

The key lives in the app's secure storage, the share sheet, the save
panel and the file associations are the app's, and the zip and Ed25519
code is Dart (`archive`, `cryptography`, pure Dart). The daemon would need
new Rust dependencies for either, and stays unchanged.

`ClaxApi` gains (`lib/clax/api.dart`, `http_api.dart`, `fake_api.dart`):

```dart
/// Version [n]'s file [path], as bytes; null when it has none.
Future<Uint8List?> readFileBytes(String aid, int n, String path);

/// One file of a publish or a draft: text, or bytes sent as base64, with
/// an optional content type.
class FileEntry { FileEntry.text(String s, {String? type}); FileEntry.bytes(Uint8List b, {String? type}); }
```

`PublishRequest.entries: Map<String, FileEntry?>` sits beside `files`
(text, unchanged for the agent), and `createDraft` takes `entries` too;
both are sent as `{content, encoding, content_type}`.

## 8. Install and update

### 8.1 Install (`lib/bundle/installer.dart`)

1. `createArtifact` with the app session (base §3.6, so the recipient's
   agent can change it), `title`, `icon` word, `capabilities`, every file
   as an entry with its manifest type, `label: "v<N> from <who>"`, `note:
   "Installed v<N> of <title> from <who>"`. A 400 `invalid_capabilities`
   is "This app needs a newer Clax".
2. Read the new version: its `content_sha256` must equal the manifest's,
   else the artifact is deleted and the install fails (`bad_file`).
3. Seed, when present: `batchDocs` `set` writes, 50 a call, no pins.
4. The lineage entry (`role: installed`, `released: [content hash]`,
   `from_bundle: true`) and the kept bundle are written last. A failure
   in 1–3 deletes the artifact.

No check runs on a first install: there is no data of the recipient's to
protect.

### 8.2 Update (`lib/bundle/updater.dart`)

Refused while the agent is building or changing the app ("Wait for the
change in progress to land"), from the make service's busy state.

1. `base` = the live version. `createDraft(of: aid, title, capabilities,
   entries)`: the update's files with a copy of the recipient's documents
   (`docs_truncated` noted).
2. **Migrate**, when `files` has `migrate.js`: the inspector loads the
   draft and runs it (§8.3) with `from` = the installed version, `to` =
   N, 30 s. An error or a timeout fails the check.
3. **Page:** `Inspector.inspect` on the draft; any uncaught error fails.
4. **Tests**, when `files` has `tests/*.test.js`: `Inspector.runTests` on
   the same draft, the render tool's limits (C2). A failed test fails;
   `not_run` fails too (an update is not trusted unchecked).
5. The draft is deleted, pass or fail.
6. On a pass, `publish(aid, if_version: base, entries)` with every file of
   the bundle and a `null` entry for each live file the bundle lacks,
   `title`, `icon`, `capabilities`, `label: "Update v<N>"`, `note:
   "Updated to v<N> from <who>"`. A 409 means the app changed during the
   check: "<title> changed while the update was checked. Try again."
7. When `migrate.js` exists, the app's documents are snapshotted (§8.4),
   then it runs once more, on the live app at the new version, in the
   migration view (§8.3). If it fails, the documents are restored from the
   snapshot and the app to `base`: `base`'s files, title, icon and
   capabilities published again, pinned to the update's version (a
   `restoreVersion` would keep the update's declaration), and the sheet says
   "The update's data step failed on your data; <title> is back on v<M>
   with your data as it was." (§9, T7). The snapshot is deleted once the
   run passes or the restore completes.
8. The lineage entry: `version` N, the new content hash appended to
   `released`, `capabilities`, `title`, `updates: on`; the kept bundle
   replaced.

Seed data in an update bundle is ignored: an update never writes the
recipient's documents except through `migrate.js`.

Capabilities: the sheet lists what the new declaration adds over the live
one (a new top-level capability, a new `net` host). Grants already follow
declarations: `drop_undeclared_grants`
(`crates/clax-core/src/store/grants.rs`) removes cross-app read grants a
declaration drops; a `net` host is reachable only while declared
(`store/net_grants.rs`); `sample` and the device capabilities resolve
`null` or `not_declared` when undeclared. A capability the person already
allowed stays allowed; an added one asks at first use, as today.

### 8.3 `migrate.js`

An app file at the root, never loaded by `index.html` (like `tests/`):

```js
// migrate.js
export default async function migrate(db, { from, to }) {
  // db: the page's window.claude.use("db"). Upgrades documents of any
  // earlier shape; it always starts from documents no run has changed.
}
```

The migration view runs it: the inspect view with `&migrate=1`, whose
content frame is named `clax-migrate` (by the shell, and by the daemon's
boot, `frame_name` in `crates/clax-server/src/boot.rs`). There the
bridge, the page's first script, writes `<plaintext hidden>` while the
document parses, so none of the app's own markup or scripts run: only
`migrate.js` writes. Host method `runMigration({file, from, to,
totalMs})`, refused outside that view, asks the bridge (`web/bridge/src/harness.ts`) to import the
module in the page's frame, call its default export with the page's `db`
and `{from, to}`, and answer host message `migration` `{ok: true}` or
`{ok: false, error}`, under the nonce, by the same rules as `tests`
(`web/shell/src/app/host.ts` header). The Dart side is
`Inspector.runMigration(Uri url, {from, to, total, cancelled})` returning
`MigrationRun {ok, error}`; it loads `url` in the migration view and, on
every outcome (answer, timeout, cancel), loads that view again and waits
for its `ready` before it returns, so no code of the run still writes.

The author's agent writes it: the system prompt (§10) says that a change
to the shape of stored documents comes with a `migrate.js` that upgrades
documents of any earlier shape. It need not be idempotent: each run, on
the draft's copy or on the live app, starts from unmigrated documents,
because a failed live run is rolled back (§8.4).

### 8.4 The document snapshot (`lib/bundle/doc_snapshot.dart`)

- **Taking it.** Every document of the app at the owner's level
  (`docs:collections`, then the paged list of each collection), written as
  `seed.jsonl`-shaped lines (§4.3) to
  `<support>/snapshots/<aid>.jsonl` through a temporary file synced and
  renamed into place, before the live run starts. No cap: it is streamed
  to the file. Its sidecar `<aid>.json` holds `base`'s version, title,
  icon and capabilities, the update's content hash and, once written, the
  version it published and the snapshot's document count; a snapshot whose
  count is missing or differs is never restored. A kept snapshot is never
  overwritten: the app takes no update until it is restored or discarded.
  The token can neither read nor write the person's private
  `data/users/<id>/` documents (`docs/contract.md`, "The db tools"), so
  the live run is also **journaled**: every `db` write of the page, through
  any handle (the shell makes every one), first sends the document's
  current value to the app, which appends `{path, before}` to
  `<support>/snapshots/<aid>.writes` and syncs it before the write goes
  ahead; a write the app could not journal fails. A rollback puts the
  owner's documents back from the snapshot and replays the journal's
  `data/users/` entries, in reverse order, through the migration view as
  the person (`restoreJournal`: `before` set, or the document deleted when
  it had none); the journal's other entries are only checked against the
  snapshot. A journal line cut short by a crash is left out: its write
  never went ahead.
- **Restoring it.** `docs:batch` in calls of 50: `delete` each document
  the app now has that the snapshot lacks, then `set` each snapshot
  document with its data (deletes first, to stay under the 5,000-document
  quota), each pinned to the version just read, one re-read and retry on
  a conflict. Document versions move forward; contents return.
- **Nothing writes meanwhile.** The update sheet is modal over the app,
  the app's page is unloaded for the whole update (its screen shows an
  "Updating…" cover), the update is refused while the agent works on the
  app and holds the agent's worker off it until it ends (§8.2), the
  migration view runs none of the app's own scripts, and
  `Inspector.runMigration` returns only once the run's document is gone
  (§8.3), so between the snapshot and the restore only `migrate.js`
  writes, and nothing once the restore starts.
- **Interrupted.** A snapshot file still present at the app's start (the
  app or the daemon died during the run or the restore) is restored then,
  with the version put back to `base` as in §8.2 step 7, only when the
  sidecar shows the live version is still the one the update published
  and no restore of it failed before; the person is told once
  ("<title>'s update was interrupted; it is back on v<M> with your data as
  it was"). Otherwise documents may have been written since, so the
  snapshot is kept and the person chooses: **Restore** (documents back to
  before the update) or **Discard**.

## 9. Threats

A bundle's app runs exactly where an agent-made app runs: the page in the
cross-origin artifact frame under the daemon's content security policy
(`connect-src 'none'`), with the shell's capabilities.

**It can reach:** its own documents; each capability it declares, behind
the existing consent: `net` to the hosts it declares once the person
allows them, `sample` on the person's API key under the per-app daily cap
(C7), and `notify`, `share`, `file`, `camera`, `location` each asked once,
each call needing the person's gesture; the person's attention (it can draw
any UI, including a fake prompt for a secret).

**It cannot reach:** other apps' documents (no `db.read`, §4.4), the API
key, the daemon token, the Flutter host channel (the shell's nonce,
base §3.4), the bundle key, files other than ones the person picks, the
network without a declared and granted host.

| # | Threat | Answer |
|---|---|---|
| T1 | A page phishes (asks for the API key, a password) | Out of reach of code; mitigated by consent: without a granted `net` host nothing leaves the device. The install sheet names every host. Accepted. |
| T2 | Prompt injection: the bundle's files steer the recipient's agent when they ask for a change | The agent of an app with `from_bundle` gets no other apps' summaries and its `db_*` tools refuse `app` (another app's ID) with "This app came from someone else; it cannot read your other apps". It still edits only that app. |
| T3 | Zip bomb | §6.1 caps; every inflate counts bytes and stops at the manifest size plus one, ignoring the zip header's sizes. |
| T4 | Path traversal, odd names | Nothing is extracted to disk (§6.3); names outside §4.1 refuse the bundle; the daemon validates paths again. |
| T5 | Tampering in transit | The signature covers the manifest, the manifest covers every file and the seed; extra or missing entries refuse it. |
| T6 | Someone else's key on an existing lineage | Installs only as a separate app (§6.4 `foreign`); never updates. |
| T7 | `migrate.js` damages the recipient's data | It runs on a copy first and the check must pass; the live run is snapshotted (§8.4), and a failure part way restores both the documents and the version. A run that succeeds but migrates wrongly in a way the update's tests did not catch stays; the person can restore the version, not the data. |
| T8 | Replay of an older signed version (downgrade) | Refused (`older`). Equal version, different content: refused (`conflict`). |
| T9 | Lost key (Clax uninstalled, new device) | The author's later bundles install as separate apps on recipients' devices. Accepted for the prototype. |
| T10 | A name that impersonates someone | Names are shown as the sender's claim beside the fingerprint. |
| T11 | Lineage squatting: a bundle reuses a known lineage ID under its own key | On an install that has the real app: a separate app. On one that does not: it pins that lineage to its key, and the real author's later bundle installs separately. Confusion only. |
| T12 | Seed data overflows or targets private paths | §4.3 caps; `data/users/` refused. |
| T13 | Personal data in the author's files | The export sheet says what goes; the agent keeps data in `db` (§10); the data switch is off by default. |
| T14 | A huge or slow update check | The check's steps have the render tool's limits; the draft copy's own caps (2,000 documents, 8 MiB) apply, and the sheet says "checked against part of your data" when it was truncated. |

## 10. The agent

`assets/prompt/system.md` gains a "Shared apps" paragraph:

> Apps may be shared with other people as a file. Never write the
> person's own data (names, numbers, notes, lists they gave you) into the
> app's files: keep it in `db`, and let the page start empty or with
> neutral examples. When a change alters the shape of stored documents,
> also write `migrate.js` (an ES module whose default export is `async
> function migrate(db, {from, to})`) that upgrades documents of any
> earlier shape. Pages never load it.

`AgentRequest` gains `fromBundle` (from the lineage store), applied as
T2 says, in `lib/agent/prompt.dart` and `lib/agent/tools/data_tools.dart`.

## 11. Migrations

None. The lineage store and the kept bundles are the app's files, like
the device grants. Were a table needed later, it would take 28 (the
branch holds 24–27 in `crates/clax-core/src/store/migrations.rs`; main
ends at 23), renumbered if another branch lands first.

## 12. Flutter UI and platform registration

### 12.1 UI (`lib/ui/bundle_sheets.dart`)

`ExportSheet`, `InstallSheet` (install, `foreign`, `forked`),
`UpdateSheet` (unmodified, modified, stopped), `BundleProgressSheet`
(build, install, the update check's steps, pass and fail with Details),
`BundleNoticeSheet` (`own`, `current`, `conflict`, `older`, refusals).
Capability words reuse `deviceCapWords`
(`lib/platform/device/device_grants.dart`) plus "use the internet:
<hosts>", "use Claude (on your API key)", "keep its own data".

- Launcher (`lib/ui/launcher.dart`): the tile menu gains **Share app**
  (Mac: **Export…**); the launcher gains **Import app…** (the app bar's
  overflow; on the Mac also File › Import App… in the menu bar).
- Inspector (`lib/ui/app_inspector.dart`) Overview: a **Share app** /
  **Export…** button, and, for an app with a lineage, "Shared as v<N>"
  or "From <who>, v<N>" with the fingerprint.
- Settings (`lib/ui/settings_screen.dart`, `lib/platform/settings_store.dart`):
  "Sharing": **Your name** (`authorName`, at most 64 characters) and the
  fingerprint once a key exists.
- `lib/app.dart` listens to `BundleIntake.opened` and shows the sheet over
  whatever screen is up (after the daemon is up).

### 12.2 Platform

- **Android** (`android/app/src/main/AndroidManifest.xml`): on
  `BundleReceiverActivity` (exported, `noHistory`, translucent), intent
  filters for `VIEW` (category `DEFAULT` and `BROWSABLE`, `scheme`
  `content`) and `SEND` (category `DEFAULT`, no scheme: its file is in
  `EXTRA_STREAM`, not the intent's data), MIME types
  `application/vnd.empathic.clax+zip`, `application/zip` and
  `application/octet-stream` (what messaging apps label an unknown file
  as). It forwards the `content:` URI, with its read grant, to the one
  `MainActivity` (singleTop) in Clax's own task (`NEW_TASK`, `SINGLE_TOP`,
  `CLEAR_TOP`) and finishes, so the sender's task never starts a second
  engine or daemon. `file:` URIs and Clax's own providers are refused.
  `BundleIntake.kt` copies the URI's bytes to
  `<cache>/incoming/<random>.clax`, refusing past 32 MiB, and hands the
  path over the channel `dev.empathic.clax/bundles` (`initial` for the
  launching intent, `opened` events from `onNewIntent`). A file that is
  not a bundle is refused by the reader with `not_a_bundle`.
- **macOS** (`macos/Runner/Info.plist`): `UTExportedTypeDeclarations`
  for `dev.empathic.clax.bundle` (extension `clax`, MIME
  `application/vnd.empathic.clax+zip`, conforming to `public.data` and
  `public.zip-archive`) and `CFBundleDocumentTypes` naming it, role
  `Viewer`, `LSHandlerRank` `Owner`. `AppDelegate.swift`'s
  `application(_:open:)` sends each URL's path on the same channel; the
  sandbox gives the app read access to a file the person opened. The save
  panel needs `files.user-selected.read-write`, which both entitlements
  files already carry.
- `lib/platform/bundle_intake.dart`: the Dart side, an interface with a
  channel implementation and a fake.

## 13. Test plan (deterministic)

| Layer | Tests |
|---|---|
| Format (`test/bundle/`) | The writer's bytes are stable (golden SHA-256 for a fixed key seed and files); the manifest round-trips; the content hash matches a vector computed from §4.5 and, in the integration test, the daemon's; Ed25519 sign and verify with RFC 8032 test vector 1; the reader refuses each case: tampered file byte, tampered manifest, bad signature, missing entry, extra entry, duplicate name, case-folded duplicate path, `../x`, `/x`, `a\b`, `a/./b`, a zip header that understates an entry's size (inflate stops at the manifest size plus one), method 12, encryption flag, zip64, format 2, unknown manifest key, `db.read` present, seed over 2,000 lines, a seed path under `data/users/`. |
| Trust (`test/bundle/trust_test.dart`) | `decide` over a table of every §6.4 row. |
| Key and store | `BundleKey` over an in-memory storage: made once, fingerprint format; `LineageStore` writes atomically, forgets on delete, version rules of §5.2, modified rules of §5.4 (restore of a release is unmodified). |
| Export, install, update | `FakeClaxApi` and `FakeInspector` (`test/support/fake_inspector.dart`, gaining `runMigration`): export with and without seed, binary files, re-share sends the kept bytes, fork mints a lineage with `forked_from`; install writes files, checks the content hash (a fake that reports another hash deletes the artifact), seeds; update passes and publishes with `null` entries for removed files; each failing step (migration error, uncaught page error, failed test, `not_run`, 409) publishes nothing and deletes the draft; a live migration failure restores the snapshot's documents (sets and deletes) and `base`; a snapshot file left at start is restored and then deleted; the snapshot covers every collection. |
| Agent | `fromBundle`: no other apps' summaries; `db_get` with `app` refused. |
| Web (vitest) | `host.ts` `runMigration` (nonce, inspect view only, latest request only, shape); `harness.ts` runs a module's default export with `db` and `{from, to}`, reports a thrown error, times out. |
| Widget | Each sheet and its buttons; the launcher's Share app and Import app…; the inspector button; Settings' name and fingerprint. |
| macOS integration (`integration_test/bundle_test.dart`) | Against the real embedded daemon and inspector: make an app (scripted), export it, install it as a second app under a fresh lineage store and key (a recipient on the same daemon), compare `content_sha256`; author publishes a change with `migrate.js` and a test, exports v2, the recipient updates and the documents are migrated; a v3 whose test fails leaves the recipient on v2. Not in the gates (`just app-integration`). |

Android intake (the intent filters and `BundleIntake.kt`) is checked on
the APK's merged manifest only; no device runs it this round, and the
verification note says so.

## 14. Tasks

Each task owns its files; none edits another's. Interfaces are this
document's, so they run in parallel; T-I runs last.

| Task | Files | Depends on |
|---|---|---|
| **BF** Format | `lib/bundle/manifest.dart`, `bundle_reader.dart`, `bundle_writer.dart`, `content_hash.dart`, `signing.dart`; `pubspec.yaml`, `pubspec.lock` (adds `archive`, `cryptography`); `test/bundle/format_*` | none |
| **BK** Key, store, trust | `lib/platform/bundle_key.dart`, `lib/bundle/lineage_store.dart`, `lib/bundle/trust.dart`; their tests | BF's `Manifest` type |
| **BA** API entries | `lib/clax/api.dart`, `http_api.dart`, `fake_api.dart`, `models.dart`; their tests | none |
| **BW** Migration runner | `web/shell/src/app/host.ts`, `web/bridge/src/harness.ts`, their tests; `lib/agent/inspector.dart`, `lib/ui/inspector_surface.dart`, `test/support/fake_inspector.dart` | none |
| **BS** Services | `lib/bundle/exporter.dart`, `installer.dart`, `updater.dart`, `doc_snapshot.dart`; their tests | BF, BK, BA, BW interfaces |
| **BU** UI | `lib/ui/bundle_sheets.dart`, `lib/ui/launcher.dart`, `lib/ui/app_inspector.dart`, `lib/ui/settings_screen.dart`, `lib/platform/settings_store.dart`, `lib/app.dart`; widget tests | BS interfaces |
| **BP** Platform | `android/app/src/main/AndroidManifest.xml`, `MainActivity.kt`, `BundleIntake.kt`, `macos/Runner/Info.plist`, `AppDelegate.swift`, `lib/platform/bundle_intake.dart` | none |
| **BG** Agent | `assets/prompt/system.md`, `lib/agent/request.dart`, `lib/agent/prompt.dart`, `lib/agent/tools/data_tools.dart`; their tests | none |
| **BD** Docs | `docs/contract.md` ("Sharing apps", `runMigration`/`migration` in "The Clax app"), `docs/usage.md` | none |
| **T-I** Integration | `integration_test/bundle_test.dart` | all |
