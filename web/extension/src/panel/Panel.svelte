<script lang="ts">
  // The side panel (spec 2026-10-05 §6.4): the active tab's live page and
  // its threads in the shell's own sidebar, who is on the page beside its
  // agents (the shell's roster), the Comment switch, the owner's name (asked
  // for only while the owner has none; spec L6, §3.1), whether the worker's
  // stream is up, and Clax's menu for the page. Every action goes to the
  // worker, which alone talks to the daemon. Everything shown from the page
  // or the daemon (titles, URLs, comments) is text, never markup.
  import Roster from "../../../shell/src/ui/Roster.svelte";
  import Sidebar from "../../../shell/src/ui/Sidebar.svelte";
  import type { Thread } from "../../../shell/src/threads";
  import { agentName } from "../../../shell/src/view/history-model";
  import { presenceMap, roster } from "../../../shell/src/view/presence-model";
  import { unsent } from "../../../shell/src/view/batch-model";
  import { type PanelState, type PanelToWorker, RETRYABLE } from "../messages";
  import { asPages, pageOfRoute } from "./adapt";

  type Link = { state: PanelState | null; up?: boolean; post(m: PanelToWorker): void };
  let { link, now }: { link: Link; now?: Date } = $props();
  const s = $derived(link.state);
  let name = $state("");
  /** The agent the person picked in a Send button's menu, while it stays on the page. */
  let chosen = $state<string | null>(null);
  const agents = $derived(s?.participants?.agents ?? []);
  const live = $derived(agents.find(a => a.live) ?? null);
  const sendTo = $derived(agents.some(a => a.handle === chosen) ? chosen : (live?.handle ?? null));
  const sendHarness = $derived(agents.find(a => a.handle === sendTo)?.harness ?? null);
  const threads = $derived(s ? asPages(s.threads) : []);
  const people = $derived(roster(s?.participants?.people ?? [], s?.presence ?? []));
  const host = $derived.by(() => { try { return s?.page ? new URL(s.page.origin).host : null; } catch { return null; } });
  const viewUrl = $derived(s?.page && /^https?:\/\//.test(s.page.url) ? s.page.url : null);
  /** The batch send's bound (spec §8). */
  const BATCH = 20;
  /** How the panel words a failure code: text, and a command to run with its copy button. */
  type Help = { before: string; command?: string; after?: string };
  const help = new Map<string, Help>([
    ["host_missing", { before: "Clax is not set up for Chrome yet. Run", command: "clax init", after: "(or /clax:extension in Claude Code), then click Retry." }],
    ["daemon_unavailable", { before: "Clax could not start. See ~/.clax/logs/daemon.log, then click Retry." }],
    ["no_capture_permission", { before: "Click the Clax button or press ⌥⇧C to comment with a screenshot." }],
  ]);
  const shownHelp = $derived(s?.error ? (help.get(s.error.code) ?? { before: s.error.message }) : null);
  /** The failure the person dismissed (by code and message), hidden until another comes. */
  let dismissed = $state<string | null>(null);
  const errorKey = $derived(s?.error ? `${s.error.code}\n${s.error.message}` : null);
  let copied = $state(false);
  function copy(text: string): void {
    void navigator.clipboard?.writeText(text).then(() => { copied = true; }, () => {});
  }
  const routeOf = (t: Thread) => t.anchor.route ?? null;
  function select(t: Thread): void {
    // A card on another route: the tab goes there first, and the overlay pins it once it re-resolves.
    if (s?.page && routeOf(t) !== s.route) link.post({ t: "navigate", route: routeOf(t), artifactId: s.page.artifact_id });
    link.post({ t: "select", threadId: t.id });
  }
  /** The name last sent, so leaving the field after Enter does not send it again. */
  let saved = "";
  function saveName(): void {
    const v = name.trim();
    if (v && v !== saved) { saved = v; link.post({ t: "set-name", name: v }); }
  }
  // A failure after a save lets the same name be sent again.
  $effect(() => { if (s?.error) saved = ""; });
</script>

<main class="panel">
  {#if !s}
    <p class="hint">Connecting to Clax…</p>
  {:else}
    <header class="head">
      <div class="title">
        <h1>{s.page?.title || "Clax"}</h1>
        {#if s.page}<p class="url">{s.page.page_url}{s.route ?? ""}</p>{/if}
      </div>
      {#if s.enabled}
        <button type="button" class="comment" class:on={s.commentMode} aria-pressed={s.commentMode}
          onclick={() => link.post({ t: "comment-mode", on: !s.commentMode })}>Comment</button>
      {/if}
    </header>
    {#if s.page && (people.length || agents.length)}
      <div class="who"><Roster {people} {agents} working={s.working} me={s.viewer?.public_id ?? null} max={5} presence={presenceMap(s.presence ?? [])} /></div>
    {/if}
    {#if link.up === false}
      <p class="notice quiet" role="status">Reconnecting to Clax. What this shows may be out of date.</p>
    {/if}
    {#if s.error && shownHelp && errorKey !== dismissed}
      <div class="notice" role="alert">
        <p>{shownHelp.before}{#if shownHelp.command}{" "}<code>{shownHelp.command}</code>{" "}{shownHelp.after ?? ""}{/if}</p>
        {#if shownHelp.command}
          {@const command = shownHelp.command}
          <button type="button" class="ghost" onclick={() => copy(command)}>{copied ? "Copied" : "Copy"}</button>
        {/if}
        {#if RETRYABLE.has(s.error.code)}
          <button type="button" onclick={() => link.post({ t: "retry" })}>Retry</button>
        {:else}
          <button type="button" class="ghost" onclick={() => (dismissed = errorKey)}>Dismiss</button>
        {/if}
      </div>
    {/if}
    {#if s.viewer && !s.viewer.display_name}
      <label class="name"><span>Your name</span>
        <input aria-label="Your name" maxlength="60" autocomplete="name" placeholder="How others see you" value={name}
          oninput={e => (name = e.currentTarget.value)} onkeydown={e => { if (e.key === "Enter") saveName(); }} onblur={saveName} />
      </label>
    {/if}
    {#if !s.enabled}
      <p class="hint">Click the Clax button or press ⌥⇧C on a page to comment on it.</p>
    {:else if s.page}
      <div class="threads">
        <Sidebar
          {threads} resolved={s.resolved} selected={s.selected} file={pageOfRoute(s.route)} {now}
          versions={s.versions} shown={s.page.current_version} agent={agentName(sendHarness)} me={s.viewer ? { ...s.viewer, created_at: "" } : null}
          working={s.working} {agents} {sendTo} commenting={s.commentMode}
          onSelect={select}
          onSend={t => link.post({ t: "send", threadId: t.id, to: sendTo })}
          onSendUnsent={() => link.post({ t: "send-batch", threadIds: unsent(s.threads).slice(0, BATCH).map(t => t.id), note: null, to: sendTo })}
          onChoose={h => (chosen = h)}
          onResolve={t => link.post({ t: t.status === "open" ? "resolve" : "reopen", threadId: t.id })}
          onReply={(t, body) => link.post({ t: "reply", threadId: t.id, body })}
          onSeen={t => link.post({ t: "looked", threadIds: [t.id] })} />
      </div>
      <footer class="foot">
        {#if viewUrl}<a href={viewUrl} target="_blank" rel="noopener noreferrer">Open in Clax</a>{/if}
        {#if host}<button type="button" class="ghost" onclick={() => link.post({ t: "turn-off", origin: s.page!.origin })}>Turn off on {host}</button>{/if}
      </footer>
    {:else}
      <p class="hint">No comments on this page yet. Press Comment, then click what you want to comment on.</p>
    {/if}
  {/if}
</main>

<style>
  :global(body) { margin: 0; background: var(--bg); color: var(--fg); }
  .panel { display: flex; flex-direction: column; min-height: 100vh; background: var(--bg); color: var(--fg); font: 14px/1.45 var(--font); }
  .head { display: flex; gap: 10px; align-items: flex-start; padding: 12px var(--gutter); border-bottom: 1px solid var(--border); background: var(--card); }
  .title { flex: 1; min-width: 0; }
  h1 { margin: 0; font-size: 15px; font-weight: 600; overflow-wrap: anywhere; }
  .url { margin: 2px 0 0; color: var(--muted); font: 12px/1.4 var(--mono); overflow-wrap: anywhere; }
  .comment.on { background: var(--you); border-color: var(--you); color: var(--on-you); }
  .who { padding: 8px var(--gutter); border-bottom: 1px solid var(--border); }
  .notice, .hint, .name { margin: 12px var(--gutter) 0; }
  .notice { display: flex; gap: 10px; align-items: center; padding: 10px 12px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--danger-tint); color: var(--fg); }
  .notice p { flex: 1; margin: 0; overflow-wrap: anywhere; }
  .notice code { font: 12.5px var(--mono); padding: 1px 5px; border-radius: var(--radius-xs); background: var(--card); border: 1px solid var(--border); white-space: nowrap; }
  .notice.quiet { background: var(--hover); color: var(--muted); font-size: 13px; }
  .hint { color: var(--muted); }
  .name { display: grid; gap: 4px; font-size: 13px; color: var(--muted); }
  .name input { width: 100%; box-sizing: border-box; }
  /* The shell's sidebar box is a fixed-width column beside the stage, and
     covers the stage on a narrow screen; in the panel it is the page's flow. */
  .threads :global(.sidebar) { position: static; width: auto; overflow: visible; border-left: 0; z-index: auto; padding: 12px var(--gutter) 18px; }
  .foot { margin-top: auto; display: flex; justify-content: space-between; align-items: center; gap: 8px; padding: 10px var(--gutter); border-top: 1px solid var(--border); font-size: 13px; }
  .foot a { color: var(--agent-ink); }
</style>
