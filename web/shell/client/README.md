# @empathic/clax-client

web/shell's code that Clax's own clients share with the shell, behind one
public surface. The Chrome extension (web/extension) imports web/shell only
through it, and web/bridge, web/overlay and web/sdk only through
[@empathic/clax-sdk](../../sdk/README.md); web/.oxlintrc.json fails any other
import.

It is AGPL-3.0-only, like web/shell, and is not published. A page outside
Clax uses @empathic/clax-sdk, which is LGPL-2.1-only and bundles none of it.
Each entry resolves to its source (scripts/client-entries.mjs) in the
extension's build, the unit tests and tsconfig.json's `paths`.

| Entry | What it gives |
|---|---|
| `@empathic/clax-client/sync` | threads and comments, the daemon's API types and `ApiError`, the inbox query, and how stream deltas (`applyThread`, `applyPresence`) and fetches (`ThreadSync`, `upsert`) fold into what a client holds. Free of Svelte and the DOM, so it is safe in a service worker. |
| `@empathic/clax-client/stream` | the live stream: `Hub`, which holds the daemon's event stream for every tab, and its messages (`HubMsg`, `TabMsg`). Free of Svelte and the DOM. |
| `@empathic/clax-client/models` | view models: presence (`presenceMap`, `roster`), agents at work, a thread's history, the sidebar's sections, the send menu and unsent batches. |
| `@empathic/clax-client/ui` | the side panel's components shown at once (`Sidebar`, `ThreadCard`, `SendButton`), `ticker` and `QuestionFeed`. |
| `@empathic/clax-client/inbox` | the inbox's and page questions' `InboxRow` and `QuestionList`, and the inbox's model (`approvalOf`, `newestSeq`): apart, for a client that loads them lazily. A component's module runs on load, so an entry a page imports at once brings every component in it. |
| `@empathic/clax-client/composer` | the comment `Composer` alone, and its `Draft`. |
| `@empathic/clax-client/roster`, `/send-menu` | components loaded lazily with `import()`, each its own chunk. |
| `@empathic/clax-client/theme.css` | the theme's stylesheet. |
| `@empathic/clax-client/testing` | for unit tests only: `mount`, the inbox's `fixtures`, and trusted events. |
