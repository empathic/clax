<svelte:options css="injected" />

<script lang="ts">
  // The people panel (spec §8, §10 "Presence"), opened from the roster: the
  // people on the artifact and the viewers present, with where they look when
  // they share it and the last version each viewed (public, decided: Q7); the
  // agents, by name and, for another person's, with their person, with what
  // they wait on (the roster's states and block times, spec 2026-10-08
  // §13, for the owner and connected people), work on or last addressed, and
  // when idle whether a comment wakes them (`wakes`); and this
  // viewer's name with the "Share where I'm looking" switch. For the
  // artifact's creator, each agent of a connected machine on it offers
  // "Raise on this artifact" (spec 2026-10-08 §7.4, O11), in a chunk of its
  // own loaded only for a creator. Escape, Close or
  // a press outside closes it; Escape and Close return focus to
  // the roster. Loaded on first open, with its styles.
  import { tick as settled, untrack } from "svelte";
  import { agentMark } from "../view/agent-display";
  import type { ArtifactController, Loaded, ViewState } from "../view/artifact-controller";
  import { personLine, roster } from "../view/presence-model";
  import { agentNames, clock, midName, newestFirst, recentlyActive, stripText, whose } from "../view/working-model";
  import { relativeTime } from "../format";
  import type { PeopleRaises, RosterAgent } from "../api";
  import type RaiseHereT from "./RaiseHere.svelte";
  import { blockedOn, waitedFor } from "../people/strip";
  import HaikuLine from "./HaikuLine.svelte";
  import { ticker } from "./ticker.svelte";
  import ViewerName from "./ViewerName.svelte";

  let { ctl, s, onClose }: { ctl: ArtifactController; s: ViewState & { data: Loaded }; onClose(): void } = $props();
  let panel: HTMLDivElement | undefined = $state();
  // Elapsed clocks tick each second while anyone works; "last here" lines each minute.
  const tick = ticker(() => s.working.length > 0, () => undefined, 60_000);
  // The agents as last refetched, as the top bar's roster shows them.
  const parts = $derived({ people: s.data.artifact.participants?.people ?? [], agents: s.agents });
  const present = $derived(new Map(s.presence.map(p => [p.public_id, p])));
  const me = $derived(s.me?.public_id ?? null);
  const RANK = { here: 0, away: 1, gone: 2 } as const;
  const people = $derived(roster(parts.people, s.presence).map((p, i) => ({ ...p, i, at: present.get(p.public_id) }))
    .sort((a, b) => Number(b.public_id === me) - Number(a.public_id === me) || (a.at ? RANK[a.at.state] : 3) - (b.at ? RANK[b.at.state] : 3) || a.i - b.i));
  const numbers = $derived(ctl.numbers(s));
  const names = $derived(agentNames(s.working, parts.agents));
  const all = $derived([...names.keys()].map(handle => {
    const a = parts.agents.find(x => x.handle === handle);
    // Ended: its session is over (not live), it works on nothing here and
    // it last acted here more than a few minutes ago (`recentlyActive`).
    // Recent: not live, but it acted here a moment ago, as an agent that
    // runs one turn per wake does. Asleep: a live agent the daemon says no
    // comment wakes while idle (`wakes: false`).
    const idle = a?.live === false && !s.working.some(w => w.agent === handle);
    const recent = idle && a && recentlyActive(a, tick.now) ? a.active_at! : null;
    return { handle, harness: a?.harness ?? s.working.find(w => w.agent === handle)?.harness ?? "agent", ended: idle && !recent, recent, asleep: !!a?.live && a.wakes === false, unsure: !!a?.live && a.wakes === "unsure" };
  }));
  const agents = $derived(all.filter(a => !a.ended));
  const earlier = $derived(all.filter(a => a.ended));
  const mine = $derived(new Set(s.attention?.open_in ?? []));
  // The agents roster (`GET /api/agents`): a LAN viewer has none.
  let listed = $state<RosterAgent[]>([]);
  $effect(() => { void fetch("/api/agents").then(r => (r.ok ? r.json() : null)).then(b => { if (Array.isArray(b?.agents)) listed = b.agents; }, () => {}); });
  // The viewer's raises (`GET /api/people/me/raises`): whether they created
  // this artifact, what they may give here, which agents a raise reaches.
  // A LAN viewer has none.
  let raises = $state<PeopleRaises | null>(null);
  let Raise: typeof RaiseHereT | null = $state(null);
  function loadRaises() {
    void fetch(`/api/people/me/raises?artifact=${encodeURIComponent(s.data.artifact.id)}`).then(r => (r.ok ? r.json() : null)).then((b: PeopleRaises | null) => {
      raises = b;
      if (b?.creator && b.holds && !Raise) void import("./RaiseHere.svelte").then(m => { Raise = m.default; }, () => {});
    }, () => {});
  }
  $effect(() => { untrack(loadRaises); });
  /** The viewer's own raise of agent `handle`, as given. */
  const mineOf = (handle: string) => raises?.raises.find(r => r.agent.handle === handle)?.access ?? null;
  const versionOf = (handle: string) => raises?.raises.find(r => r.agent.handle === handle)?.version ?? null;
  const personOf = (handle: string) => parts.agents.find(a => a.handle === handle)?.person;
  // Another person's agent, named with them; another Clax's agent's name already says whose Clax it relays from.
  const theirs = (handle: string) => { const p = personOf(handle); return !!p && p.public_id !== me && (!!me || !p.owner) && !parts.agents.find(a => a.handle === handle)?.via; };
  /** What the roster says an agent waits on: its person (you, for yours),
   * since when, and on what. Only an agent the roster calls `waiting` (an
   * open question or `ask`, spec 2026-10-08 §13) waits, so every viewer
   * reads the same; approval requests alone waiting on you say so. */
  function waiting(handle: string, name: string): string | null {
    const r = listed.find(x => x.handle === handle) ?? listed.find(x => !x.handle && x.name === name && x.live);
    if (!r) return null;
    const on = r.blocked && blockedOn(r.blocked) ? ` (${blockedOn(r.blocked)})` : "";
    if (r.state === "waiting" && r.since) return `Waiting on ${theirs(handle) ? midName(personOf(handle)!.name) : "you"} ${waitedFor(r.blocked?.since ?? r.since, tick.now)}${on}`;
    return r.blocked?.approvals && !theirs(handle) ? `${blockedOn({ ...r.blocked, questions: 0 })} for you · ${waitedFor(r.blocked.since, tick.now)}` : null;
  }
  const initials = (n: string | null) => {
    const w = (n ?? "?").trim().split(/\s+/);
    return (w.length > 1 ? w[0][0] + w[1][0] : w[0].slice(0, 2)).toUpperCase();
  };
  const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;
  /** The open threads `pid` commented on. */
  const threadsOf = (pid: string) => s.threads.filter(t => t.status === "open" && t.comments.some(c => c.author_public_id === pid)).length;
  function personNote(p: { public_id: string; seen: number | null }): string {
    const n = threadsOf(p.public_id);
    return [n ? `In ${plural(n, "thread")}.` : "", p.seen !== null ? `Seen v${p.seen}.` : ""].filter(Boolean).join(" ");
  }
  /** What an agent works on, without its name: `On #1 (yours) and #3`, or its message. */
  function onWhat(text: string, name: string): string {
    if (text.startsWith(`${name} is working on `)) return `On ${text.slice(`${name} is working on `.length)}`;
    if (text.startsWith(`${name}: `)) return text.slice(name.length + 2);
    return "Working";
  }
  /** An idle (or, `ended`, an ended) agent's line: the newest version it published that addressed threads. */
  function idleLine(handle: string, ended = false): string {
    const state = ended ? "Ended." : "Idle.";
    const v = [...s.data.versions].reverse().find(x => x.agent === handle && x.addresses?.length);
    if (!v) return state;
    const ids = v.addresses ?? [];
    const what = ids.every(t => numbers.has(t)) && ids.length <= 3 ? ids.map(t => `#${numbers.get(t)}`).join(", ") : plural(ids.length, "thread");
    return `${state} Addressed ${what} in v${v.n}.`;
  }
  const who = () => document.querySelector<HTMLElement>("button.who");
  /** The card head of the thread the name is asked for (`s.askName`). */
  const askedHead = () => [...document.querySelectorAll<HTMLElement>(".thread-card")].find(c => c.dataset.thread === s.askName)?.querySelector<HTMLElement>(".card-head");
  // Asked for a name to reopen a thread: closed with none given, the reopen
  // is forgotten, and focus goes back to that thread's card.
  function close() {
    const back = s.askName ? askedHead() : null;
    if (s.askName && !s.me?.display_name && !panel?.querySelector<HTMLInputElement>(".viewer-name")?.value.trim()) ctl.forgetReopen();
    onClose();
    (back ?? who())?.focus();
  }
  // Once the thread is open again (its name saved), the prompt has done its
  // work: it closes, and focus goes to the thread's card in Open.
  $effect(() => {
    const id = s.askName;
    if (!id || s.threads.find(t => t.id === id)?.status !== "open") return;
    onClose();
    void settled().then(() => [...document.querySelectorAll<HTMLElement>(".thread-card")].find(c => c.dataset.thread === id)?.querySelector<HTMLElement>(".card-head")?.focus());
  });
  function outside(e: PointerEvent) {
    const t = e.target as Element;
    // A press in another dialog (a page's consent prompt over the shell) leaves it open.
    if (!panel || panel.contains(t) || t.closest?.("button.who, [role=dialog], [role=alertdialog]")) return;
    // Focus stays where the press put it: moving it by script after a press
    // elsewhere (a consent dialog's Allow) would be shell input of its own.
    onClose();
  }
  // On wide screens the panel opens under the roster, kept inside the bar.
  let left: string | undefined = $state();
  $effect(() => {
    // Opened to ask for the name: focus goes into the name field.
    const field = untrack(() => s.askName) ? panel?.querySelector<HTMLInputElement>(".viewer-name") : null;
    (field ?? panel)?.focus();
    const b = who();
    const bar = b?.offsetParent as HTMLElement | null;
    if (b && bar && matchMedia("(min-width: 701px)").matches) left = `${Math.max(16, Math.min(b.offsetLeft, bar.clientWidth - 436))}px`;
  });
</script>

<!-- Back on the page, a creator's raises are read again: someone else may have removed one. -->
<svelte:window onpointerdown={outside} onfocus={() => { if (raises?.creator) loadRaises(); }} />
<!-- Escape closes the dialog. -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="people" role="dialog" aria-label="People and agents" tabindex="-1" bind:this={panel} style:left
  onkeydown={e => { if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); close(); } }}>
  <h3 class="ph"><span class="sw"></span>People · {people.length}</h3>
  {#each people as p (p.public_id)}
    {@const line = p.at ? personLine(p.at, tick.now) : "not here"}
    {@const note = personNote(p)}
    <div class="prow">
      <span class={["tok p", p.public_id === me && "me", p.at?.state === "here" ? "here" : p.at && "away"]} aria-hidden="true">{initials(p.display_name)}</span>
      <b>{p.display_name ?? "Viewer"} <small>{p.public_id === me ? `you · ${line}` : line}</small></b>
      {#if note}<p class="muted">{note}</p>{/if}
    </div>
  {/each}
  <h3 class="ah"><span class="sw"></span>Agents · {agents.length}</h3>
  {#each agents as a (a.handle)}{@render agentRow(a)}{/each}
  {#if earlier.length}
    <h3 class="ah eh"><span class="sw"></span>Earlier · {earlier.length}</h3>
    {#each earlier as a (a.handle)}
      {@const name = names.get(a.handle) ?? a.harness}
      <div class="prow agent ended">
        <span class="tok a" aria-hidden="true">{agentMark(names.get(a.handle), a.harness)}</span>
        <b title={personOf(a.handle) ? `${whose(personOf(a.handle)!.name)} agent` : undefined}>{theirs(a.handle) ? `${name} · ${midName(personOf(a.handle)!.name)}` : name}</b>
        <p class="muted">{idleLine(a.handle, true)}</p>
      </div>
    {/each}
  {/if}
  {#snippet agentRow(a: { handle: string; harness: string; recent: string | null; asleep: boolean; unsure: boolean })}
    {@const name = names.get(a.handle) ?? a.harness}
    {@const w = newestFirst(s.working).find(x => x.agent === a.handle)}
    {@const held = waiting(a.handle, name)}
    <div class="prow agent">
      <span class={["tok a", w && "work"]} aria-hidden="true">{agentMark(names.get(a.handle), a.harness)}</span>
      <b title={personOf(a.handle) ? `${whose(personOf(a.handle)!.name)} agent` : undefined}>{theirs(a.handle) ? `${name} · ${midName(personOf(a.handle)!.name)}` : name}</b>
      {#if held}
        <p class="on">{held}</p>
      {:else if w}
        <p><span class="on">{onWhat(stripText(w, names, numbers, mine), name)}</span> · {clock(w.started_at, tick.now)}</p>
        {#if !s.commenting}<HaikuLine seed={w.key} />{/if}
      {:else if a.recent}
        <p>Active {relativeTime(a.recent, tick.now)}.</p>
      {:else}
        <p>{idleLine(a.handle)}</p>
        {#if a.asleep}<p class="nowake">Won't be woken by your comments until it next acts.</p>
        {:else if a.unsure}<p class="nowake">May not be woken by your comments: Claude Code does not confirm its channel.</p>{/if}
      {/if}
      {#if Raise && raises?.holds && raises.raisable?.includes(a.handle)}
        <Raise aid={s.data.artifact.id} handle={a.handle} {name} holds={raises.holds} mine={mineOf(a.handle)} version={versionOf(a.handle)} onChanged={loadRaises} />
      {/if}
    </div>
  {/snippet}
  <div class="prow edit">
    {#if s.askName}<p class="ask" role="status">Add your name to reopen the thread: people see it beside what you do.</p>{/if}
    <span>You are {#if s.me?.display_name}<b class="nm">{s.me.display_name}</b>{/if}</span>
    <ViewerName setNotice={ctl.setNotice} onViewer={v => ctl.setMe(v)} />
    <label class="share"><input type="checkbox" checked={s.shareWhere} onchange={e => ctl.setShareWhere(e.currentTarget.checked)} /> Share where I'm looking</label>
    <button type="button" class="ghost" onclick={close}>Close</button>
  </div>
</div>

<!-- The panel's styles travel with its lazy chunk. -->
<style>
  :global {
    .people { position: absolute; top: 56px; left: 16px; z-index: 20; width: 420px; max-height: 70vh; overflow: auto; background: var(--raised); border: 1px solid var(--border-hover); border-radius: var(--radius); box-shadow: var(--elev-lg); padding: 6px 0 8px; }
    .people h3 { margin: 10px 16px 6px; font: 600 13px var(--font); color: var(--muted); display: flex; align-items: center; gap: 8px; }
    .people h3 .sw { width: 7px; height: 14px; } .people .eh .sw { opacity: .45; } .prow.ended .tok { opacity: .55; } .people .ph .sw { border-radius: 0 7px 7px 0; background: var(--you); } .people .ah .sw { border-radius: 7px 0 0 7px; background: var(--agent); }
    .prow.edit .ask { flex-basis: 100%; margin: 0; padding: 8px 10px; border-radius: var(--radius-sm); background: var(--accent-tint); color: var(--fg); font-size: 13px; line-height: 1.45; }
    .prow { display: grid; grid-template-columns: 52px 1fr; gap: 2px 10px; padding: 7px 16px; align-items: start; }
    .prow b { font: 600 14px/1.25 var(--font); } .prow b small { font: 400 12px var(--font); color: var(--muted); margin-left: 6px; }
    .prow p { grid-column: 2; margin: 0; font-size: 12.5px; line-height: 1.45; } .prow .tok { grid-row: span 2; justify-self: start; }
    .prow .hk { grid-column: 2; }
    .prow.edit { border-top: 1px solid var(--border); margin-top: 6px; padding-top: 10px; display: flex; flex-wrap: wrap; gap: 8px; align-items: center; font-size: 12px; color: var(--muted); }
    @media (max-width: 700px) { .people { position: fixed; left: 0; right: 0; top: 56px; bottom: 52px; width: auto; max-height: none; box-shadow: none; border-width: 1px 0 0; border-radius: 0; } }
    .people:focus { outline: none; }
    .prow p.muted { color: var(--muted); } .prow .on { color: var(--agent-ink); font-weight: 600; }
    .prow p.nowake { color: var(--muted); font-style: italic; }
    .prow .hk { font-size: 12px; color: var(--muted); line-height: 1.5; margin-top: 2px; }
    .prow.edit .nm { font: 600 14px var(--font); color: var(--fg); }
    .prow.edit .viewer-name { flex: 1; min-width: 140px; }
    .prow.edit .share { flex-basis: 100%; display: flex; align-items: center; gap: 6px; color: var(--fg); }
    .prow.edit button.ghost { margin-left: auto; }
  }
</style>
