<svelte:options css="injected" />

<script lang="ts">
  // One capability's grants on an admin page (spec 2026-11-19-claxbot-browser
  // §4.3, §11.2): the admin's switch offers a capability and grants it to no
  // one; this lists who holds it, "Everyone on this instance" first, then each
  // person, each with Revoke, and grants it to everyone or to one person.
  import { refusal } from "../people/refusal";
  import { type AdminGrant, type AdminPerson, type CapabilityId, grantCapability, revokeGrant } from "./api";

  let { capability, label, grants, people, onchange }: {
    capability: CapabilityId;
    /** What the capability is called on the page. */
    label: string;
    /** Every live admin grant; this card shows those of `capability`. */
    grants: AdminGrant[];
    people: AdminPerson[];
    onchange: (grants: AdminGrant[]) => void;
  } = $props();

  let to = $state("everyone");
  let busy = $state(false);
  let err = $state<string | null>(null);
  const id = $derived(`cap-${capability.replace(/[.:]/g, "-")}`);
  const mine = $derived(grants.filter(g => g.capability === capability).sort((a, b) => Number(a.grantee === "person") - Number(b.grantee === "person")));
  const nameOf = (person: string | null) => people.find(p => p.person === person)?.name ?? person ?? "";
  const held = (key: string) => mine.some(g => (key === "everyone" ? g.grantee === "everyone" : g.person === key));

  async function act(fn: () => Promise<{ grants: AdminGrant[] }>) {
    busy = true; err = null;
    try { onchange((await fn()).grants); } catch (e) { err = refusal(e); } finally { busy = false; }
  }
</script>

<section class="capcard" aria-labelledby={`${id}-h`}>
  <h4 id={`${id}-h`}>Who may use {label}</h4>
  {#if mine.length}
    <ul class="capgrants">
      {#each mine as g (g.id)}
        <li>
          <span>{g.grantee === "everyone" ? "Everyone on this instance" : nameOf(g.person)}{#if g.granter === "config"}<small class="note">{" · from config.toml"}</small>{:else if g.conveys === false}<small class="note">{` · conveys nothing: ${nameOf(g.granter ?? null)} is no longer an admin`}</small>{/if}</span>
          <button type="button" aria-disabled={busy} aria-label={`Revoke ${label} from ${g.grantee === "everyone" ? "everyone on this instance" : nameOf(g.person)}`} onclick={() => { if (!busy) void act(() => revokeGrant(g.id)); }}>Revoke</button>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="note">Granted to no one: switched on, it reaches no run until you grant it.</p>
  {/if}
  <div class="capgrant">
    <label for={`${id}-to`}>Grant to</label>
    <select id={`${id}-to`} bind:value={to}>
      <option value="everyone" disabled={held("everyone")}>Everyone on this instance</option>
      {#each people as p (p.person)}<option value={p.person} disabled={held(p.person)}>{p.name ?? p.person}</option>{/each}
    </select>
    <button type="button" aria-disabled={busy || held(to)} onclick={() => { if (!busy && !held(to)) void act(() => grantCapability(capability, to)); }}>Grant</button>
  </div>
  {#if err}<p class="err" role="alert">{err}</p>{/if}
</section>

<style>
  :global {
    .capcard { display: grid; gap: 8px; margin-top: 12px; max-width: 560px; }
    .capcard h4 { margin: 0; font-size: 14px; }
    .capgrants { list-style: none; margin: 0; padding: 0; display: grid; gap: 6px; }
    .capgrants li { display: flex; justify-content: space-between; align-items: center; gap: 8px; min-width: 0; }
    .capgrant { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; }
  }
</style>
