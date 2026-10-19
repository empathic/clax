<svelte:options css="injected" />

<script lang="ts">
  // The gallery's New dialog (docs/contract.md "Starting an artifact from a
  // brief"), loaded on the first press. First a title and a brief: Create
  // makes the artifact, with a placeholder page and the brief as its first
  // thread. Then who builds it: the live agents this browser may send to,
  // grouped as the Send menu groups them; picking one sends it the brief,
  // which gives it a watch on the artifact (another person's agent waits
  // for that person's approval). With no live agent, a prompt to paste into
  // one, and the list appears as soon as an agent connects. Every way out
  // of the second step lands on the new artifact. A modal dialog:
  // everything else is inert while it is open, Tab stays inside, Escape or
  // Close closes it and focus goes back.
  import { onMount } from "svelte";
  import { nav } from "../nav";
  import { connectPrompt, formProblem, liveAgents, personNames, pickGroups, sendBrief, start, type Me, type PickGroup, type Started } from "../view/new-artifact";
  import { inertOutside, trapTab } from "./modal";

  let { me, onClose }: { me: Me; onClose(): void } = $props();
  const back = document.activeElement as HTMLElement | null;
  let panel: HTMLElement | undefined = $state();
  let backdrop: HTMLElement | undefined = $state();
  let title = $state("");
  let brief = $state("");
  let busy = $state(false);
  let error = $state("");
  let tried = $state(false);
  let started = $state<Started | null>(null);
  // The agents to pick from: null while the first list loads.
  let groups = $state<PickGroup[] | null>(null);
  let sending = $state<string | null>(null);
  let copied = $state(false);
  let poll: ReturnType<typeof setInterval> | undefined;
  let copyTimer: ReturnType<typeof setTimeout> | undefined;
  const problem = $derived(formProblem(title, brief));
  const prompt = $derived(started ? connectPrompt(new URL(started.url, location.origin).href, started.artifact.title) : "");
  const focus = (el: HTMLElement) => { el.focus(); };

  onMount(() => {
    const undo = backdrop ? inertOutside(backdrop) : () => {};
    return () => {
      undo();
      clearInterval(poll);
      clearTimeout(copyTimer);
      if (back?.isConnected) back.focus?.();
    };
  });

  /** The live agents, grouped, with other people named. */
  async function agents(): Promise<PickGroup[]> {
    const list = await liveAgents();
    const others = list.map(a => a.person).filter((p): p is string => !!p && p !== me?.public_id);
    return pickGroups(list, me, await personNames(others));
  }

  async function create(e: SubmitEvent) {
    e.preventDefault();
    tried = true;
    if (problem || busy) return;
    busy = true;
    error = "";
    try {
      started = await start(title, brief);
      groups = await agents();
      // With none live, look again until one connects.
      if (!groups.length) poll = setInterval(() => { void agents().then(g => { if (g.length) { groups = g; clearInterval(poll); } }, () => {}); }, 3000);
    } catch (x) {
      error = x instanceof Error ? x.message : "The artifact was not made";
    } finally { busy = false; }
  }

  /** Lands on the new artifact. */
  function open() { if (started) nav.assign(started.url); }

  async function pick(to: string) {
    if (!started || sending) return;
    sending = to;
    error = "";
    try {
      await sendBrief(started.artifact.id, started.thread.id, to);
      open();
    } catch (x) {
      error = x instanceof Error ? x.message : "The brief was not sent";
      sending = null;
    }
  }

  function close() { if (started) open(); else onClose(); }

  function copy() {
    void navigator.clipboard?.writeText(prompt).then(() => {
      copied = true;
      clearTimeout(copyTimer);
      copyTimer = setTimeout(() => { copied = false; }, 1600);
    }, () => { copied = false; });
  }

  function keydown(e: KeyboardEvent) {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); close(); return; }
    // The dialog's keys are its own, never the gallery's.
    if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) e.stopPropagation();
    if (panel) trapTab(e, panel);
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="na-backdrop" bind:this={backdrop} onclick={e => { if (e.target === e.currentTarget && !started && !busy) onClose(); }}>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="na-panel" role="dialog" aria-modal="true" aria-labelledby="na-title" tabindex="-1" bind:this={panel} onkeydown={keydown}>
    {#if !started}
      <header>
        <h2 id="na-title">New artifact</h2>
        <button type="button" class="ghost" onclick={onClose}>Close</button>
      </header>
      <p class="lede">Say what you want built. The artifact starts as a placeholder page, and your brief becomes its first comment, for the agent you pick next.</p>
      <form onsubmit={create} novalidate>
        <label for="na-name">Title</label>
        <input id="na-name" type="text" autocomplete="off" maxlength="200" placeholder="Q3 revenue chart" bind:value={title} {@attach focus} />
        <label for="na-brief">Brief</label>
        <textarea id="na-brief" rows="6" placeholder="A bar chart of Q3 revenue by region, sorted high to low, with last quarter as a faint ghost bar." bind:value={brief}></textarea>
        {#if error}<p class="error" role="alert">{error}</p>{:else if tried && problem}<p class="error" role="alert">{problem}</p>{/if}
        <div class="acts">
          <button type="button" class="ghost" onclick={onClose}>Cancel</button>
          <button type="submit" class="primary" aria-disabled={busy}>{busy ? "Creating…" : "Create"}</button>
        </div>
      </form>
    {:else}
      <header>
        <h2 id="na-title">Who builds it?</h2>
        <button type="button" class="ghost" onclick={close}>Close</button>
      </header>
      <p class="made">“{started.artifact.title}” is ready for an agent: its brief is its first comment.</p>
      {#if groups === null}
        <p class="lede" role="status">Looking for agents…</p>
      {:else if groups.length}
        <p class="lede">Pick an agent. It watches the artifact and gets your brief; another person's agent starts once that person approves.</p>
        <div class="groups">
          {#each groups as g (g.label)}
            <section aria-label={g.label}>
              <h3>{g.label}</h3>
              <ul>
                {#each g.agents as a (a.to)}
                  <li>
                    <button type="button" class="agent" aria-disabled={!!sending} {@attach el => { if (g === groups![0] && a === g.agents[0]) el.focus(); }} onclick={() => pick(a.to)}>
                      <span class="dot" aria-hidden="true"></span>
                      <span class="nm">{a.name}</span>
                      {#if a.who}<span class="who">{a.who}</span>{/if}
                      <span class="go">{sending === a.to ? "Sending…" : "Send brief"}</span>
                    </button>
                  </li>
                {/each}
              </ul>
            </section>
          {/each}
        </div>
        {#if error}<p class="error" role="alert">{error}</p>{/if}
        <div class="acts"><button type="button" class="ghost" onclick={open}>Not now: open it</button></div>
      {:else}
        <p class="lede"><strong>No agent is connected.</strong> Paste this into one (Claude Code with the Clax plugin, say) and it will watch this artifact and build it from your brief. Once it connects, it is listed here.</p>
        <div class="cf">
          <div class="cf-head"><span class="cf-label">Prompt</span><button type="button" class="small" aria-label="Copy the prompt" onclick={copy}>{copied ? "Copied" : "Copy"}</button></div>
          <pre aria-label="Prompt">{prompt}</pre>
        </div>
        <div class="acts"><button type="button" class="primary" {@attach focus} onclick={open}>Open the artifact</button></div>
        <span class="sr" aria-live="polite">{copied ? "Prompt copied" : ""}</span>
      {/if}
    {/if}
  </div>
</div>

<!-- The dialog's styles travel with its lazy chunk (injected on mount), so
     the gallery's eager CSS does not carry them. -->
<style>
  .na-backdrop { position: fixed; inset: 0; z-index: 40; background: var(--scrim); display: grid; place-items: center; padding: var(--gutter, 16px); }
  .na-panel { background: var(--raised); color: var(--fg); border: 1px solid var(--border-hover); border-radius: var(--radius); box-shadow: var(--elev-lg); width: min(560px, 100%); max-height: calc(100dvh - 32px); overflow: auto; padding: 16px 20px 20px; box-sizing: border-box; }
  header { display: flex; align-items: center; gap: 12px; margin-bottom: 6px; }
  h2 { margin: 0; font-size: 17px; flex: 1; }
  h3 { margin: 0 0 6px; font-size: 12px; font-weight: 600; color: var(--muted); text-transform: uppercase; letter-spacing: .04em; }
  .lede { margin: 0 0 14px; font-size: 13px; color: var(--muted); line-height: 1.6; }
  .lede strong { color: var(--fg); font-weight: 600; }
  .made { margin: 0 0 10px; font-size: 14px; line-height: 1.5; overflow-wrap: anywhere; }
  form { display: grid; gap: 6px; }
  label { font-size: 12.5px; font-weight: 600; color: var(--muted); margin-top: 6px; }
  input, textarea { width: 100%; box-sizing: border-box; font: 14px/1.5 var(--font); color: var(--fg); background: var(--bg); border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 7px 10px; }
  textarea { resize: vertical; min-height: 120px; }
  input:focus-visible, textarea:focus-visible { outline: 2px solid var(--focus); outline-offset: 1px; border-color: transparent; }
  .acts { display: flex; justify-content: flex-end; gap: 8px; margin-top: 14px; }
  .groups { display: grid; gap: 14px; }
  ul { list-style: none; margin: 0; padding: 0; display: grid; gap: 6px; }
  .agent { width: 100%; justify-content: flex-start; gap: 10px; padding: 9px 12px; text-align: left; }
  .agent .dot { width: 8px; height: 8px; border-radius: 50%; background: var(--agent); flex: none; }
  .agent .nm { font-weight: 600; overflow: hidden; text-overflow: ellipsis; min-width: 0; }
  .agent .who { color: var(--muted); font-weight: 400; font-size: 12.5px; overflow: hidden; text-overflow: ellipsis; min-width: 0; }
  .agent .go { margin-left: auto; font-size: 12.5px; color: var(--muted); flex: none; }
  .agent:not([aria-disabled="true"]):hover .go { color: var(--fg); }
  .cf { display: grid; gap: 4px; min-width: 0; }
  .cf-head { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
  .cf-label { font-size: 12.5px; color: var(--muted); font-weight: 500; }
  .small { min-height: 26px; padding: 3px 10px; font-size: 12.5px; }
  pre { display: block; margin: 0; font: 12.5px/1.55 var(--mono); background: var(--hover); border: 1px solid var(--border); border-radius: var(--radius-xs); padding: 8px 10px; color: var(--fg); overflow-wrap: anywhere; white-space: pre-wrap; user-select: all; }
  .error { margin: 8px 0 0; font-size: 12.5px; color: var(--danger); }
  .sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); }
  @media (max-width: 700px) {
    .na-panel { padding: 14px 14px 16px; }
    .acts { flex-direction: column-reverse; }
    .acts button { width: 100%; }
    .agent { flex-wrap: wrap; }
    .agent .go { margin-left: 18px; flex-basis: 100%; }
  }
</style>
