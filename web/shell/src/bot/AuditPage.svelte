<svelte:options css="injected" />

<script lang="ts">
  // `/audit` (spec 2026-10-21-claxbot §9.2): the audit log, newest first, as
  // the daemon lets this person see it (admins: everything; anyone else: what
  // they did, on what they own, and runs for them). Filters (text, who
  // acted, kind group, artifact, time range) are the address's query, so a
  // link from a run panel or Admin opens it filtered; each change reads the
  // first page again, and More reads the next by its cursor. Choosing an
  // event shows its stored body beside the list (below it at phone width),
  // with links to its run and artifact. Export Toolpath downloads the
  // Toolpath export of the artifact and time range chosen (the owner's).
  import { onMount } from "svelte";
  import { listArtifacts } from "../api";
  import { refusal } from "../people/refusal";
  import { type AdminPerson, type AuditDetail, type AuditEvent, type AuditFilter, type AuditGroup, adminPeople, audit, auditEvent, exportToolpath } from "./api";
  import { isAdmin, myBot, myRole } from "./me";
  import { GROUPS, RANGES, type Range, actorLabel, rangeFrom } from "./model";
  import BotPage from "./BotPage.svelte";

  let { now }: { now?: () => Date } = $props();
  const url = new URLSearchParams(location.search);
  let q = $state(url.get("q") ?? "");
  let actor = $state(url.get("actor") ?? "");
  let group = $state(url.get("group") ?? "");
  let artifact = $state(url.get("artifact") ?? "");
  let range = $state<Range>((RANGES.find(r => r.id === url.get("range"))?.id) ?? "7d");
  let events = $state<AuditEvent[] | null>(null);
  let next = $state<number | null>(null);
  let err = $state<string | null>(null);
  let loading = $state(false);
  let detail = $state<AuditDetail | null>(null);
  let artifacts = $state<{ id: string; title: string }[]>([]);
  let people = $state<AdminPerson[]>([]);
  let exporting = $state(false);
  // Whether the export route admits this person (`can_export`, `GET /api/me/bot`).
  let canExport = $state(false);
  // Each read is numbered: an answer to an older filter is dropped.
  let ticket = 0;
  const filter = (): AuditFilter => ({ q: q.trim() || undefined, actor: actor || undefined, group: (group || undefined) as AuditGroup | undefined, artifact: artifact || undefined, from: rangeFrom(range, now?.() ?? new Date()) });
  function remember() {
    const p = new URLSearchParams();
    if (q.trim()) p.set("q", q.trim());
    if (actor) p.set("actor", actor);
    if (group) p.set("group", group);
    if (artifact) p.set("artifact", artifact);
    if (range !== "7d") p.set("range", range);
    const s = p.toString();
    history.replaceState(history.state, "", `/audit${s ? `?${s}` : ""}`);
  }
  async function load(more = false) {
    const t = ++ticket;
    loading = true; err = null;
    if (!more) remember();
    try {
      const page = await audit({ ...filter(), cursor: more ? next ?? undefined : undefined });
      if (t !== ticket) return;
      events = more ? [...(events ?? []), ...page.events] : page.events;
      next = page.next_cursor;
    } catch (e) { if (t === ticket) err = refusal(e); } finally { if (t === ticket) loading = false; }
  }
  let typing: ReturnType<typeof setTimeout> | undefined;
  const typed = () => { clearTimeout(typing); typing = setTimeout(() => { void load(); }, 300); };
  let detailEl: HTMLElement | undefined = $state();
  // An event opened: the detail takes the focus, and comes into view (below the list at phone width).
  let focusDetail = false;
  $effect(() => {
    if (!detail || !detailEl || !focusDetail) return;
    focusDetail = false;
    detailEl.focus({ preventScroll: true });
    detailEl.scrollIntoView?.({ block: "nearest" });
  });
  async function open(e: AuditEvent) {
    try { detail = await auditEvent(e.seq); focusDetail = true; } catch (x) { err = refusal(x); }
  }
  async function exp() {
    exporting = true; err = null;
    try {
      const blob = await exportToolpath({ artifact: artifact || undefined, from: rangeFrom(range, now?.() ?? new Date()) });
      const a = document.createElement("a");
      a.href = URL.createObjectURL(blob);
      a.download = `clax-toolpath${artifact ? `-${artifact}` : ""}.json`;
      a.click();
      setTimeout(() => URL.revokeObjectURL(a.href), 0);
    } catch (x) { err = `The export did not run: ${refusal(x)}`; } finally { exporting = false; }
  }
  const time = (iso: string) => new Date(iso).toLocaleString("en-US", { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit", second: "2-digit" });
  const title = (id: string | null) => (id ? artifacts.find(a => a.id === id)?.title ?? id : null);
  const body = (b: unknown) => JSON.stringify(b, null, 2);
  onMount(() => {
    document.title = "Audit · Clax";
    void load();
    void listArtifacts().then(l => { artifacts = l.map(a => ({ id: a.id, title: a.title })); }, () => {});
    void myBot().then(b => { canExport = !!b?.can_export; });
    void myRole().then(r => { if (isAdmin(r)) void adminPeople().then(p => { people = p; }, () => {}); });
    return () => clearTimeout(typing);
  });
</script>

{#snippet actions()}{#if canExport}<button type="button" disabled={exporting} onclick={exp}>{exporting ? "Exporting…" : "Export Toolpath"}</button>{/if}{/snippet}

<BotPage title="Audit" lede="Every view and every change, by people, admins and ClaxBot, as the Toolpath journal records them." {actions}>
  <form class="aud-f" role="search" aria-label="Filter the audit log" onsubmit={e => { e.preventDefault(); void load(); }}>
    <input type="search" aria-label="Search the audit log" placeholder="Search" bind:value={q} oninput={typed} />
    <select aria-label="Who acted" onchange={e => { actor = e.currentTarget.value; void load(); }}>
      <option value="" selected={!actor}>Everyone</option>
      {#each people as p (p.person)}<option value={p.person} selected={actor === p.person}>{p.name ?? p.person}</option>{/each}
      <option value="claxbot" selected={actor === "claxbot"}>ClaxBot</option>
    </select>
    <select aria-label="Kind" onchange={e => { group = e.currentTarget.value; void load(); }}>{#each GROUPS as g (g.id)}<option value={g.id} selected={group === g.id}>{g.label}</option>{/each}</select>
    <select aria-label="Artifact" onchange={e => { artifact = e.currentTarget.value; void load(); }}>
      <option value="" selected={!artifact}>Any artifact</option>
      {#if artifact && !artifacts.some(a => a.id === artifact)}<option value={artifact} selected>{artifact}</option>{/if}
      {#each artifacts as a (a.id)}<option value={a.id} selected={artifact === a.id}>{a.title}</option>{/each}
    </select>
    <select aria-label="When" onchange={e => { range = e.currentTarget.value as Range; void load(); }}>{#each RANGES as r (r.id)}<option value={r.id} selected={range === r.id}>{r.label}</option>{/each}</select>
  </form>
  {#if err}<p class="err" role="alert">{err}</p>{/if}
  <div class="aud" class:with={!!detail}>
    <section class="bcard aud-l" aria-label="Events" aria-busy={loading}>
      {#if events?.length}
        <ul>
          {#each events as e (e.seq)}
            <li><button type="button" class="ghost" aria-pressed={detail?.seq === e.seq} onclick={() => open(e)}>
              <span class="t">{time(e.at)}</span><span class="who">{actorLabel(e)}</span><code>{e.kind}</code><span class="sum">{e.summary}{#if e.artifact_id && title(e.artifact_id)} · {title(e.artifact_id)}{/if}</span><span class="seq">#{e.seq}</span>
            </button></li>
          {/each}
        </ul>
        {#if next}<div class="acts"><button type="button" disabled={loading} onclick={() => load(true)}>{loading ? "Loading…" : "More"}</button></div>{/if}
      {:else if events}
        <p class="note">Nothing matches.</p>
      {:else if !err}
        <p class="note" role="status">Loading…</p>
      {/if}
    </section>
    {#if detail}
      <section class="bcard aud-d" aria-labelledby="aud-d-h" tabindex="-1" bind:this={detailEl}>
        <header><h3 id="aud-d-h"><code>{detail.kind}</code> · #{detail.seq}</h3><button type="button" class="ghost" onclick={() => { detail = null; }}>Close</button></header>
        <p>{actorLabel(detail)}{#if detail.artifact_id}, on {title(detail.artifact_id)}{/if}{#if detail.run}, in run {detail.run}{/if}. Recorded {time(detail.at)}.</p>
        <p class="sumd">{detail.summary}</p>
        <pre aria-label="Stored body">{body(detail.body)}</pre>
        <div class="acts">
          {#if detail.run && detail.artifact_id}<a class="lnk" href={`/a/${encodeURIComponent(detail.artifact_id)}#run=${encodeURIComponent(detail.run)}`}>Open the run</a>{/if}
          {#if detail.artifact_id}<a class="lnk" href={`/a/${encodeURIComponent(detail.artifact_id)}`}>Open the artifact</a>{/if}
          {#if detail.run}<button type="button" onclick={() => { q = detail!.run!; void load(); }}>Same run only</button>{/if}
        </div>
      </section>
    {/if}
  </div>
</BotPage>

<style>
  :global {
    .aud-f { display: flex; flex-wrap: wrap; gap: 8px; }
    .aud-f input { flex: 1 1 200px; min-width: 0; }
    .aud-f select { flex: 0 1 auto; min-width: 0; max-width: 100%; }
    .aud { display: grid; gap: 14px; min-width: 0; }
    .aud.with { grid-template-columns: minmax(0, 1.4fr) minmax(0, 1fr); align-items: start; }
    .aud-l ul { list-style: none; margin: 0; padding: 0; }
    .aud-l li + li { border-top: 1px solid var(--border); }
    .aud-l button { width: 100%; display: grid; grid-template-columns: auto auto minmax(0, 1fr) auto; grid-template-areas: "t who kind seq" "sum sum sum sum"; justify-content: stretch; gap: 2px 10px; text-align: start; white-space: normal; border-radius: var(--radius-xs); padding: 8px 6px; }
    .aud-l button[aria-pressed="true"] { background: var(--accent-tint); }
    .aud-l .t { grid-area: t; font: 12px var(--mono); color: var(--muted); font-variant-numeric: tabular-nums; }
    .aud-l .who { grid-area: who; font-weight: 600; font-size: 13px; overflow-wrap: anywhere; }
    .aud-l code { grid-area: kind; font: 12px var(--mono); color: var(--accent-ink); overflow-wrap: anywhere; }
    .aud-l .seq { grid-area: seq; font: 12px var(--mono); color: var(--muted); }
    .aud-l .sum { grid-area: sum; font-weight: 400; font-size: 13px; color: var(--fg); overflow-wrap: anywhere; }
    .aud-d header { display: flex; align-items: center; gap: 8px; }
    .aud-d h3 { flex: 1; margin: 0; overflow-wrap: anywhere; }
    .aud-d .sumd { color: var(--fg); }
    .aud-d pre { margin: 0 0 10px; font: 12px/1.55 var(--mono); background: var(--hover); border: 1px solid var(--border); border-radius: var(--radius-xs); padding: 8px 10px; white-space: pre-wrap; overflow-wrap: anywhere; max-height: 50vh; overflow: auto; }
    .bpg a.lnk { color: var(--accent-ink); font-weight: 600; font-size: 13.5px; }
    @media (max-width: 860px) { .aud.with { grid-template-columns: minmax(0, 1fr); } }
  }
</style>
