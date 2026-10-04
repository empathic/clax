<svelte:options css="injected" />

<script lang="ts">
  // The people panel (spec §8, §10 "Presence"), opened from the roster: the
  // people on the artifact and the viewers present, with where they look when
  // they share it and the last version each viewed (public, decided: Q7); the
  // agents, with what they work on or what they last addressed; and this
  // viewer's name with the "Share where I'm looking" switch. Escape, Close or
  // a press outside closes it; Escape and Close return focus to
  // the roster. Loaded on first open, with its styles.
  import type { ArtifactController, Loaded, ViewState } from "../view/artifact-controller";
  import { personLine, roster } from "../view/presence-model";
  import { agentNames, clock, newestFirst, stripText } from "../view/working-model";
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
  const agents = $derived([...names.keys()].map(handle => ({ handle, harness: parts.agents.find(a => a.handle === handle)?.harness ?? s.working.find(w => w.agent === handle)?.harness ?? "agent" })));
  const mine = $derived(new Set(s.attention?.open_in ?? []));
  const AGENT: Record<string, string> = { claude: "cl", codex: "cx", pi: "pi" };
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
  /** An idle agent's line: the newest version it published that addressed threads. */
  function idleLine(handle: string): string {
    const v = [...s.data.versions].reverse().find(x => x.agent === handle && x.addresses?.length);
    if (!v) return "Idle.";
    const ids = v.addresses ?? [];
    const what = ids.every(t => numbers.has(t)) && ids.length <= 3 ? ids.map(t => `#${numbers.get(t)}`).join(", ") : plural(ids.length, "thread");
    return `Idle. Addressed ${what} in v${v.n}.`;
  }
  const who = () => document.querySelector<HTMLElement>("button.who");
  function close() { onClose(); who()?.focus(); }
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
    panel?.focus();
    const b = who();
    const bar = b?.offsetParent as HTMLElement | null;
    if (b && bar && matchMedia("(min-width: 701px)").matches) left = `${Math.max(16, Math.min(b.offsetLeft, bar.clientWidth - 436))}px`;
  });
</script>

<svelte:window onpointerdown={outside} />
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
  {#each agents as a (a.handle)}
    {@const name = names.get(a.handle) ?? a.harness}
    {@const w = newestFirst(s.working).find(x => x.agent === a.handle)}
    <div class="prow">
      <span class={["tok a", w && "work"]} aria-hidden="true">{AGENT[a.harness] ?? a.harness.slice(0, 2)}</span>
      <b>{name}</b>
      {#if w}
        <p><span class="on">{onWhat(stripText(w, names, numbers, mine), name)}</span> · {clock(w.started_at, tick.now)}</p>
        {#if !s.commenting}<HaikuLine seed={w.key} />{/if}
      {:else}
        <p>{idleLine(a.handle)}</p>
      {/if}
    </div>
  {/each}
  <div class="prow edit">
    <span>You are {#if s.me?.display_name}<b class="nm">{s.me.display_name}</b>{/if}</span>
    <ViewerName setNotice={ctl.setNotice} onViewer={v => ctl.setMe(v)} />
    <label class="share"><input type="checkbox" checked={s.shareWhere} onchange={e => ctl.setShareWhere(e.currentTarget.checked)} /> Share where I'm looking</label>
    <button type="button" class="ghost" onclick={close}>Close</button>
  </div>
</div>

<!-- The panel's styles travel with its lazy chunk. -->
<style>
  :global {
    .people { position: absolute; top: 56px; left: 16px; z-index: 20; width: 420px; max-height: 70vh; overflow: auto; background: var(--raised); border: 1px solid var(--border-strong); box-shadow: 0 14px 40px var(--shadow); padding: 6px 0 8px; }
    .people h3 { margin: 10px 16px 6px; font: 600 14px var(--grot); color: var(--muted); display: flex; align-items: center; gap: 8px; }
    .people h3 .sw { width: 7px; height: 14px; } .people .ph .sw { border-radius: 0 7px 7px 0; background: var(--you); } .people .ah .sw { border-radius: 7px 0 0 7px; background: var(--agent); }
    .prow { display: grid; grid-template-columns: 52px 1fr; gap: 2px 10px; padding: 7px 16px; align-items: start; }
    .prow b { font: 600 15px/1.2 var(--grot); } .prow b small { font: 400 11.5px var(--mono); color: var(--muted); margin-left: 6px; }
    .prow p { grid-column: 2; margin: 0; font-size: 12.5px; line-height: 1.45; } .prow .tok { grid-row: span 2; justify-self: start; }
    .prow .hk { grid-column: 2; }
    .prow.edit { border-top: 1px solid var(--border); margin-top: 6px; padding-top: 10px; display: flex; flex-wrap: wrap; gap: 8px; align-items: center; font-size: 12px; color: var(--muted); }
    @media (max-width: 700px) { .people { position: fixed; left: 0; right: 0; top: 56px; bottom: 52px; width: auto; max-height: none; box-shadow: none; border-width: 1px 0 0; } }
    .people:focus { outline: none; }
    .prow p.muted { color: var(--muted); } .prow .on { color: var(--agent-ink); font-weight: 600; }
    .prow .hk { font-size: 11.5px; color: var(--muted); line-height: 1.5; margin-top: 2px; }
    .prow.edit .nm { font: 600 14px var(--grot); color: var(--fg); }
    .prow.edit .viewer-name { flex: 1; min-width: 140px; }
    .prow.edit .share { flex-basis: 100%; display: flex; align-items: center; gap: 6px; color: var(--fg); }
    .prow.edit button.ghost { margin-left: auto; }
  }
</style>
