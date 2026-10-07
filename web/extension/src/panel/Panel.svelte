<script lang="ts">
  // The side panel (spec 2026-10-05 §6.4): the active tab's live page and
  // its threads in the shell's own sidebar, who is on the page beside its
  // agents (the shell's roster), the Comment switch, the owner's name (asked
  // for only while the owner has none; spec L6, §3.1), whether the worker's
  // stream is up, and the way to turn Clax off in the tab. Below the page's
  // threads, the threads of the site's other pages (each opened in place to
  // read and answer it there, the tab staying put), a status filter and a
  // search over both (remembered per site), moving a thread to another page,
  // and merging pages (spec §7.1, owner decision 2026-10-06). Every action
  // goes to the worker, which alone talks to the daemon. Everything shown
  // from the page or the daemon (titles, URLs, comments) is text, never markup.
  import Roster from "../../../shell/src/ui/Roster.svelte";
  import Sidebar from "../../../shell/src/ui/Sidebar.svelte";
  import type { Thread } from "../../../shell/src/threads";
  import { agentName } from "../../../shell/src/view/history-model";
  import { presenceMap, roster } from "../../../shell/src/view/presence-model";
  import { unsent } from "../../../shell/src/view/batch-model";
  import { sidebarSections } from "../../../shell/src/view/sidebar-model";
  import { type FarPage, type PanelState, type PanelToWorker, RETRYABLE, type SiteChoice, type SiteView, type Suggestion } from "../messages";
  import { asPages, pageOfRoute } from "./adapt";
  import Elsewhere from "./Elsewhere.svelte";
  import Merge from "./Merge.svelte";
  import SiteTools from "./SiteTools.svelte";
  import MoveTo from "./MoveTo.svelte";
  import { type Shortcut, offHint, panelHint, readShortcut } from "../shortcut";
  import { DEFAULT_PREFS, type Prefs, loadPrefs, savePrefs } from "./prefs";
  import { FILTERS, type Filter, groups, matches, pageLabel } from "./site-model";

  type Step = { moved: number; remaining: number };
  type Ask = { t: "rule"; origin: string; pattern: string } | { t: "unrule"; origin: string; ruleId: string }
    | { t: "join"; origin: string; with: string } | { t: "split"; origin: string };
  type Link = {
    state: PanelState | null; up?: boolean; site?: SiteView | null; failures?: number; post(m: PanelToWorker): void; request?(m: Ask): Promise<Step>;
    clip?(threadId: string): Promise<string | null>;
    farPage?(threadId: string): Promise<FarPage | null>;
    suggestion?: { origin: string; suggestion: Suggestion | null } | null; sites?: SiteChoice[] | null;
  };
  type Area = Parameters<typeof loadPrefs>[0];
  const local = (): Area => { try { return chrome.storage.local; } catch { return undefined; } };
  /** Asks Chrome for the origins' permission, as the panel's click allows (none to ask for in tests). */
  const chromePermit = (origins: string[]): Promise<boolean> => {
    try { return chrome.permissions.request({ origins: origins.map(o => `${o}/*`) }).catch(() => false); } catch { return Promise.resolve(true); }
  };
  /** `shortcut`: the keyboard command's shortcut, which the panel names when Comment needs a grant. */
  let { link, now, store = local(), permit = chromePermit, shortcut = readShortcut() }: { link: Link; now?: Date; store?: Area; permit?(origins: string[]): Promise<boolean>; shortcut?: Promise<Shortcut> } = $props();
  let keys = $state<Shortcut>(null);
  $effect(() => { void shortcut.then(k => { keys = k; }); });
  const s = $derived(link.state);
  const site = $derived(link.site ?? null);
  let name = $state("");
  /** The agent the person picked in a Send button's menu, while it stays on the page. */
  let chosen = $state<string | null>(null);
  const agents = $derived(s?.participants?.agents ?? []);
  const live = $derived(agents.find(a => a.live) ?? null);
  const sendTo = $derived(agents.some(a => a.handle === chosen) ? chosen : (live?.handle ?? null));
  const sendHarness = $derived(agents.find(a => a.handle === sendTo)?.harness ?? null);
  const threads = $derived(s ? asPages(s.threads) : []);
  /** The tab's site, whose filter and collapsed groups are remembered. */
  const origin = $derived(site?.origin ?? s?.page?.origin ?? (s?.url ? new URL(s.url).origin : null));
  let prefs = $state<Prefs>({ ...DEFAULT_PREFS });
  let prefsFor: string | null = null;
  /** How often the person changed the prefs, so a late read of the kept ones does not undo a change. */
  let changes = 0;
  $effect(() => {
    const o = origin;
    if (!o || o === prefsFor) return;
    prefsFor = o;
    prefs = { ...DEFAULT_PREFS };
    const at = ++changes;
    // What the person changed meanwhile wins over what was kept.
    void loadPrefs(store, o).then(p => { if (prefsFor === o && changes === at) prefs = p; });
  });
  const setPrefs = (p: Prefs) => { changes++; prefs = p; if (origin) savePrefs(store, origin, p); };
  const setFilter = (filter: Filter) => setPrefs({ ...prefs, filter });
  const toggleGroup = (label: string, open: boolean) =>
    setPrefs({ ...prefs, collapsed: open ? prefs.collapsed.filter(c => c !== label) : [...prefs.collapsed.filter(c => c !== label), label] });
  let search = $state("");
  const here = $derived(s?.page?.artifact_id ?? null);
  /** The page's threads that pass the filter and the search, numbered as the pins are over all of them. */
  const shown = $derived(threads.filter(t => matches(t, prefs.filter, search, t.page_path ?? s?.page?.path)));
  const numbers = $derived(sidebarSections(threads, s?.resolved ?? {}, pageOfRoute(s?.route ?? null)).numbers);
  const far = $derived(groups(site, here, prefs.filter, search));
  const anyFar = $derived((site?.pages ?? []).some(p => p.page.artifact_id !== here && p.threads.length));
  const filtering = $derived(prefs.filter !== "all" || !!search.trim());
  /** Where a thread can be moved: the tab's page (unless it is there), and the site's other pages. */
  function targets(t: Thread): { url: string; label: string }[] {
    const out: { url: string; label: string }[] = [];
    if (s?.url && t.artifact_id !== here) out.push({ url: s.url, label: `This page (${new URL(s.url).pathname})` });
    for (const p of site?.pages ?? []) if (p.page.artifact_id !== t.artifact_id && p.page.artifact_id !== here) out.push({ url: p.page.page_url, label: pageLabel(p.page) });
    return out;
  }
  const move = (t: Thread, url: string) => link.post({ t: "move", threadId: t.id, pageUrl: url });
  /** The page's thread selected (on its pin or its card), which can be moved from here. */
  const picked = $derived(s?.threads.find(t => t.id === s.selected) ?? null);
  let movingPicked = $state(false);
  $effect(() => { if (!picked) movingPicked = false; });
  const ask = (m: Ask): Promise<Step> => link.request?.(m) ?? Promise.reject(new Error("Clax cannot do that here."));
  /** The origin Clax is on for in the tab, which the site's tools act for. */
  const tabOrigin = $derived(s?.enabled && s.url ? new URL(s.url).origin : null);
  /** Asked once per origin whose site is loaded and joins nothing: whether it may be the same app as another (spec §7.2). */
  let askedFor: string | null = null;
  $effect(() => {
    const o = tabOrigin;
    if (!o || !site || site.origin !== o || site.site?.joined || askedFor === o) return;
    askedFor = o;
    link.post({ t: "suggest" });
  });
  /** A thread of the site opens on any of its addresses: Chrome is asked for
   * the others under the click (no prompt for those it allows already). */
  function openThread(t: Thread): void {
    const others = (site?.site?.origins ?? []).map(o => o.origin).filter(o => o !== tabOrigin);
    const asked = others.length ? permit(others) : Promise.resolve(true);
    void asked.then(() => link.post({ t: "open-thread", threadId: t.id }));
  }
  /** The clips last shown (at most CLIPS), asked of the worker once each while kept (the panel cannot fetch them). */
  const CLIPS = 20;
  const clips = new Map<string, Promise<string | null>>();
  function clip(t: Thread): Promise<string | null> {
    const key = `${t.id} ${t.clip_url}`;
    let p = clips.get(key);
    if (p) clips.delete(key);
    else {
      p = link.clip?.(t.id) ?? Promise.resolve(null);
      // A clip that did not come is asked for again next time.
      void p.then(u => { if (u === null && clips.get(key) === p) clips.delete(key); });
    }
    clips.set(key, p);
    for (const k of clips.keys()) { if (clips.size <= CLIPS) break; clips.delete(k); }
    return p;
  }
  /** The site's other pages whose threads are open in place, by artifact ID: their live agents and versions. */
  let farPages = $state<Record<string, FarPage>>({});
  /** The site's threads open in place, by ID, for the panel's life. */
  let unfolded = $state<string[]>([]);
  const unfold = (t: Thread) => { void link.farPage?.(t.id).then(p => { if (p) farPages = { ...farPages, [p.artifactId]: p }; }); };
  const toggleResolved = (t: Thread) => link.post({ t: t.status === "open" ? "resolve" : "reopen", threadId: t.id });
  const reply = (t: Thread, body: string) => link.post({ t: "reply", threadId: t.id, body });
  const suggestion = $derived(link.suggestion && link.suggestion.origin === tabOrigin ? link.suggestion.suggestion : null);
  const people = $derived(roster(s?.participants?.people ?? [], s?.presence ?? []));
  const viewUrl = $derived(s?.page && /^https?:\/\//.test(s.page.url) ? s.page.url : null);
  /** The batch send's bound (spec §8). */
  const BATCH = 20;
  /** How the panel words a failure code: text, and a command to run with its copy button. */
  type Help = { before: string; command?: string; after?: string };
  const help = $derived(new Map<string, Help>([
    ["host_missing", { before: "Clax is not set up for Chrome yet. Run", command: "clax init", after: "(or /clax:extension in Claude Code), then click Retry." }],
    ["daemon_unavailable", { before: "Clax could not start. See ~/.clax/logs/daemon.log, then click Retry." }],
    ["no_capture_permission", { before: panelHint(keys) }],
  ]));
  const shownHelp = $derived(s?.error ? (help.get(s.error.code) ?? { before: s.error.message }) : null);
  /** The failure the person dismissed, by code, message and how many
   * actions had failed: hidden until another comes, or an action fails again. */
  let dismissed = $state<string | null>(null);
  const errorKey = $derived(s?.error ? `${s.error.code}\n${s.error.message}\n${link.failures ?? 0}` : null);
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
        {#if s.page}{@const address = s.page.page_url + (s.route ?? "")}<p class="url" title={address}>{address}</p>{/if}
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
    {#if tabOrigin && site && suggestion}
      {#key tabOrigin}
        <SiteTools {site} origin={tabOrigin} {suggestion} sites={null} request={ask} {permit} banner
          answer={(w, a) => link.post({ t: "answer", with: w, answer: a })} list={() => {}} />
      {/key}
    {/if}
    {#if !s.enabled}
      <p class="hint">{offHint(keys)}</p>
    {:else}
      {#if s.page || anyFar}
        <div class="tools">
          <div class="seg" role="group" aria-label="Show threads">
            {#each FILTERS as f (f.value)}
              <button type="button" aria-pressed={prefs.filter === f.value} onclick={() => setFilter(f.value)}>{f.label}</button>
            {/each}
          </div>
          <input type="search" aria-label="Search comments" placeholder="Search text, people or paths" value={search} oninput={e => (search = e.currentTarget.value)} />
        </div>
      {/if}
      {#if anyFar}<h2 class="sect">This page</h2>{/if}
    {/if}
    {#if !s.enabled}
      <!-- told above -->
    {:else if s.page}
      {#if picked}
        <div class="picked">
          {#if movingPicked}
            <MoveTo targets={targets(picked)} onMove={url => { movingPicked = false; move(picked, url); }} onCancel={() => (movingPicked = false)} />
          {:else}
            <button type="button" class="ghost" onclick={() => (movingPicked = true)}>Move the selected thread to another page…</button>
          {/if}
        </div>
      {/if}
      {#if filtering && !shown.length}<p class="hint">Nothing on this page matches.</p>{:else}
      <div class="threads">
        <Sidebar
          threads={shown} {numbers} resolved={s.resolved} selected={s.selected} file={pageOfRoute(s.route)} {now}
          versions={s.versions} shown={s.page.current_version} agent={agentName(sendHarness)} me={s.viewer ? { ...s.viewer, created_at: "" } : null}
          working={s.working} {agents} {sendTo} commenting={s.commentMode}
          onSelect={select}
          onSend={t => link.post({ t: "send", threadId: t.id, to: sendTo })}
          onSendUnsent={() => link.post({ t: "send-batch", threadIds: unsent(s.threads).slice(0, BATCH).map(t => t.id), note: null, to: sendTo })}
          onChoose={h => (chosen = h)}
          onResolve={toggleResolved} onReply={reply} {clip}
          onSeen={t => link.post({ t: "looked", threadIds: [t.id] })} />
      </div>
      {/if}
    {:else}
      <p class="hint">No comments on this page yet. Press Comment, then click what you want to comment on.</p>
    {/if}
    {#if s.enabled && anyFar}
      <Elsewhere groups={far} resolved={s.resolved} selected={s.selected} collapsed={prefs.collapsed} {now} {targets}
        me={s.viewer ? { ...s.viewer, created_at: "" } : null} {clip} pages={farPages} {chosen} {unfolded}
        onFold={(id, o) => (unfolded = o ? [...unfolded, id] : unfolded.filter(x => x !== id))}
        onToggle={toggleGroup} onOpen={openThread} onUnfold={unfold} onMove={move} onReply={reply} onResolve={toggleResolved}
        onSend={(t, to) => link.post({ t: "send", threadId: t.id, to })} onChoose={h => (chosen = h)}
        onSeen={t => link.post({ t: "looked", threadIds: [t.id] })} />
    {/if}
    {#if s.enabled && site}
      <!-- A run belongs to its site: another site's panel starts afresh, and the run stops. -->
      {#key site.origin}<Merge {site} request={ask} />{/key}
    {/if}
    {#if tabOrigin && site}
      {#key tabOrigin}
        <SiteTools {site} origin={tabOrigin} suggestion={null} sites={link.sites ?? null} request={ask} {permit}
          answer={(w, a) => link.post({ t: "answer", with: w, answer: a })} list={() => link.post({ t: "list-sites" })} />
      {/key}
    {/if}
    {#if s.enabled}
      <footer class="foot">
        {#if viewUrl}<a href={viewUrl} target="_blank" rel="noopener noreferrer">Open in Clax</a>{/if}
        {#if s.tabId !== null}
          {@const tabId = s.tabId}
          <button type="button" class="ghost" onclick={() => link.post({ t: "turn-off", tabId })}>Turn off in this tab</button>
        {/if}
      </footer>
    {/if}
  {/if}
</main>

<style>
  :global(body) { margin: 0; background: var(--bg); color: var(--fg); }
  .panel { display: flex; flex-direction: column; min-height: 100vh; background: var(--bg); color: var(--fg); font: 14px/1.45 var(--font); }
  .head { display: flex; gap: 10px; align-items: flex-start; padding: 12px var(--gutter); border-bottom: 1px solid var(--border); background: var(--card); }
  .title { flex: 1; min-width: 0; }
  h1 { margin: 0; font-size: 15px; font-weight: 600; overflow-wrap: anywhere; }
  .url { margin: 2px 0 0; color: var(--muted); font: 12px/1.4 var(--mono); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
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
  .tools { display: grid; gap: 8px; padding: 12px var(--gutter) 0; }
  .seg { display: flex; border: 1px solid var(--border-strong); border-radius: var(--radius-sm); overflow: hidden; }
  .seg button { flex: 1; min-width: 0; min-height: 28px; padding: 4px 6px; border: 0; border-radius: 0; background: var(--card); color: var(--muted); font-size: 12.5px; }
  .seg button + button { border-left: 1px solid var(--border); }
  .seg button[aria-pressed="true"] { background: var(--fg); color: var(--bg); }
  .seg button[aria-pressed="true"]:not(:disabled):hover { background: var(--primary-hover); }
  .tools input { width: 100%; min-width: 0; font-size: 13px; }
  .sect { margin: 14px var(--gutter) 0; font: 600 12px/1.2 var(--font); color: var(--muted); text-transform: uppercase; letter-spacing: .04em; }
  .picked { margin: 10px var(--gutter) 0; }
  .picked > button { width: 100%; min-height: 28px; font-size: 12.5px; color: var(--muted); border: 1px dashed var(--border-hover); }
  .foot { margin-top: auto; display: flex; justify-content: space-between; align-items: center; gap: 8px; padding: 10px var(--gutter); border-top: 1px solid var(--border); font-size: 13px; }
  .foot a { color: var(--agent-ink); }
  .foot button { margin-left: auto; }
</style>
