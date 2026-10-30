<svelte:options css="injected" />

<script lang="ts">
  // Admin › People & flags (spec 2026-10-21-claxbot §5, §7.4): every
  // connected person with their role (the owner changes it; the owner's own
  // row cannot), whether they may use ClaxBot, whose key their runs use,
  // their weekly credits (the free tier and the lift an admin grants, with a
  // reason the audit records), this week's use against it, and whether the
  // limit is enforced. Each change goes to the daemon and the list is read
  // again; a refusal shows as the daemon words it.
  import { onMount } from "svelte";
  import { refusal } from "../people/refusal";
  import { type AdminPerson, type Role, adminPeople, setFlags, setLift, setRole } from "./api";
  import { credits, meter, tokens } from "./model";
  import Meter from "./Meter.svelte";
  import Switch from "./Switch.svelte";

  let { role }: { role: Role } = $props();
  let people = $state<AdminPerson[] | null>(null);
  let err = $state<string | null>(null);
  let busy = $state<string | null>(null);
  // The person whose lift is being edited, and the form's fields.
  let editing = $state<string | null>(null);
  let lift = $state("");
  let reason = $state("");
  let liftEl: HTMLInputElement | undefined = $state();
  $effect(() => { liftEl?.focus(); });
  const owner = $derived(role === "owner");
  const KEY = { own: "Own key", instance: "Instance" } as const;
  const stats = $derived(people && {
    admins: people.filter(p => p.role !== "member").length,
    on: people.filter(p => p.flags.claxbot).length,
    spent: people.reduce((n, p) => n + p.week.spent_milli, 0),
    tokens: people.reduce((n, p) => n + p.tokens_week, 0),
    runs: people.reduce((n, p) => n + (p.runs_week ?? 0), 0),
    refused: people.reduce((n, p) => n + (p.refused_week ?? 0), 0),
  });
  const load = () => adminPeople().then(l => { people = l; }, e => { err = refusal(e); });
  async function act(who: string, fn: () => Promise<unknown>) {
    if (busy) return;
    busy = who; err = null;
    try { await fn(); await load(); } catch (e) { err = refusal(e); await load(); } finally { busy = null; }
  }
  /** Changes `p`'s role from its select `el`; refused, the select shows the role again. */
  async function changeRole(p: AdminPerson, el: HTMLSelectElement) {
    const to = el.value as "admin" | "member";
    await act(p.person, () => setRole(p.person, to));
    const now = people?.find(x => x.person === p.person)?.role ?? p.role;
    if (el.value !== now) el.value = now;
  }
  /** The plan: the free tier alone, or lifted. */
  const plan = (p: AdminPerson) => (p.week.lift_milli ? `Lifted +${credits(p.week.lift_milli)}` : "Free");
  const name = (p: AdminPerson) => p.name ?? p.person;
  function edit(p: AdminPerson) { editing = p.person; lift = String(p.week.lift_milli / 1000); reason = ""; }
  function saveLift(e: SubmitEvent, p: AdminPerson) {
    e.preventDefault();
    const n = Number(lift);
    if (!Number.isFinite(n) || n < 0) { err = "A lift is a number of credits, 0 or more."; return; }
    if (!reason.trim()) { err = "Say why: the audit log records it."; return; }
    void act(p.person, async () => { await setLift(p.person, Math.round(n * 1000), reason.trim()); editing = null; });
  }
  onMount(() => { void load(); });
</script>

{#if err}<p class="err" role="alert">{err}</p>{/if}
{#if stats}
  <div class="stats">
    <div class="stat"><b>{stats.admins}</b><span>Admins</span></div>
    <div class="stat"><b>{stats.on} of {people!.length}</b><span>ClaxBot on</span></div>
    <div class="stat"><b>{credits(stats.spent)}</b><span>Credits this week</span></div>
    <div class="stat"><b>{tokens(stats.tokens)}</b><span>Tokens this week</span></div>
    <div class="stat"><b>{stats.runs}</b><span>Runs this week</span></div>
    <div class="stat"><b>{stats.refused}</b><span>Refused at the limit</span></div>
  </div>
{/if}
{#if people}
  <section class="bcard" aria-labelledby="ap-h">
    <h3 id="ap-h">People</h3>
    <div class="scroll">
      <table class="ap">
        <thead><tr><th>Person</th><th>Role</th><th>ClaxBot</th><th>Model key</th><th>Plan</th><th class="num">Weekly credits</th><th>This week</th><th>Unenforced</th></tr></thead>
        <tbody>
          {#each people as p (p.person)}
            {@const m = meter({ ...p.week, start: "", end: "", warn: false, exhausted: false })}
            <tr>
              <td class="nm"><b>{name(p)}</b></td>
              <td data-l="Role">
                {#if p.role === "owner"}Owner · admin
                {:else if owner}
                  <select aria-label={`Role of ${name(p)}`} disabled={busy === p.person} onchange={e => changeRole(p, e.currentTarget)}>
                    <option value="admin" selected={p.role === "admin"}>Admin</option><option value="member" selected={p.role === "member"}>Member</option>
                  </select>
                {:else}{p.role === "admin" ? "Admin" : "Member"}{/if}
              </td>
              <td data-l="ClaxBot"><Switch checked={p.flags.claxbot} label={`ClaxBot for ${name(p)}`} hideLabel busy={busy === p.person} onchange={on => act(p.person, () => setFlags(p.person, { claxbot: on }))} /></td>
              <td data-l="Model key">{KEY[p.key_source]}</td>
              <td data-l="Plan">{plan(p)}</td>
              <td class="num" data-l="Weekly credits">
                {#if editing === p.person}
                  <form class="lift" onsubmit={e => saveLift(e, p)}>
                    <label>Lift <input type="number" min="0" step="1" inputmode="numeric" bind:value={lift} bind:this={liftEl} /></label>
                    <label>Reason <input type="text" maxlength="200" placeholder="beta tester" bind:value={reason} /></label>
                    <span class="acts"><button type="submit" class="primary" aria-disabled={busy === p.person}>Save</button><button type="button" class="ghost" onclick={() => { editing = null; }}>Cancel</button></span>
                  </form>
                {:else}
                  {credits(p.week.weekly_milli)}
                  <button type="button" class="ghost" aria-label={`Change the lift of ${name(p)}`} onclick={() => edit(p)}>Lift</button>
                {/if}
              </td>
              <td data-l="This week"><div class="wk"><Meter {m} small label={`${name(p)}'s credits this week`} /><small>{credits(p.week.spent_milli)} · {tokens(p.tokens_week)} tok</small></div></td>
              <td data-l="Unenforced"><Switch checked={p.flags.claxbot_unenforced} label={`Unenforced limit for ${name(p)}`} hideLabel busy={busy === p.person} onchange={on => act(p.person, () => setFlags(p.person, { claxbot_unenforced: on }))} /></td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    <p class="note">Unenforced: their use is metered and shown, and never stops a run.</p>
  </section>
{:else if !err}
  <p class="note" role="status">Loading people…</p>
{/if}

<style>
  :global {
    .ap { min-width: 760px; }
    .ap .wk { display: grid; gap: 4px; min-width: 110px; }
    .ap .wk small, .ap .liftn { color: var(--muted); font-size: 12px; }
    .ap .lift { display: grid; gap: 6px; justify-items: start; text-align: start; }
    .ap .lift label { display: grid; gap: 2px; font-size: 12px; color: var(--muted); }
    .ap .lift input { width: 140px; }
    /* At phone width each person is a block of labelled lines, not a row to scroll. */
    @media (max-width: 700px) {
      .ap { min-width: 0; }
      .ap thead { display: none; }
      .ap, .ap tbody, .ap tr, .ap td { display: block; }
      .ap tr { padding: 8px 0; border-bottom: 1px solid var(--border); }
      .ap tr:last-child { border-bottom: 0; }
      .ap td { display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 4px 0; border: 0; text-align: start; }
      .ap td.num { text-align: start; white-space: normal; }
      .ap td[data-l]::before { content: attr(data-l); font-size: 12px; font-weight: 600; color: var(--muted); flex: none; }
      .ap td.nm { font-size: 15px; }
      .ap .wk { min-width: 0; flex: 0 1 180px; }
    }
  }
</style>
