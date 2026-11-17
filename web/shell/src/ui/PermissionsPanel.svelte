<svelte:options css="injected" />
<script lang="ts">
  // The Permissions panel (the more menu's Permissions…): everything the page
  // may use, one row each with its state and how to take it back
  // (permissions-model.ts), for a viewer the daemon says may approve. Revoke
  // calls the row's route and then reads the answer again; the routes that
  // revoke the internet, Claude and another app's data take the owner's
  // token, so a viewer without one sees those rows without Revoke. A row
  // waiting for approval offers Review, which opens the approval dialog
  // (ApprovalDialog). Withdrawing an approval changes what the page is
  // served, so the view reloads (ArtifactController.permissionsChanged); the
  // panel then opens again on the new document (the flag `reopenKey`). Loaded after the
  // first paint (`more-menu.svelte.ts`), which keeps its opener; opened at
  // `"policy"` (the blocked notice's Permissions…) it focuses the Content
  // Security Policy setting. Under the rows, the artifact's Content Security
  // Policy, where the daemon says it: for whoever administers it
  // (`may_tag`) the control the hosted Share dialog shows too
  // (ContentSecurityPolicySetting, the same control and words), its change
  // made with the browser's cookie through the shell's confirmation flow
  // (`hosted/api.ts` `setContentSecurityPolicy`, `held.ts`); for anyone else
  // who opens the panel, a line when the clax is served unrestricted.
  import { onMount, tick, untrack } from "svelte";
  import { fromStore } from "svelte/store";
  import { getToken } from "../api";
  import type { ArtifactController } from "../view/artifact-controller";
  import { failureText, readError, readJson } from "../failure";
  import { type ContentSecurityPolicy, setContentSecurityPolicy } from "../hosted/api";
  import { policyOf } from "../view/artifact-controller";
  import ApprovalDialog from "./ApprovalDialog.svelte";
  import { CSP_HEADING, CSP_NOTE, SELF_NOTE } from "./content-security-policy-model";
  import ContentSecurityPolicySetting from "./ContentSecurityPolicySetting.svelte";
  import { setPermissionsOpener } from "./more-menu.svelte";
  import { type PermissionRow, type PermissionsView, permissionRows } from "./permissions-model";
  import { inertOutside, trapTab } from "./modal";

  let { ctl }: { ctl: ArtifactController } = $props();
  // svelte-ignore state_referenced_locally
  const view = fromStore(ctl.state);
  const s = $derived(view.current);
  // svelte-ignore state_referenced_locally
  const reopenKey = `clax.permissions.reopen:${ctl.id}`;

  let open = $state(false);
  let answer = $state<PermissionsView | null>(null);
  let rows = $state<PermissionRow[]>([]);
  let token = $state<string | null>(null);
  let working = $state<string | null>(null);
  let error = $state<string | null>(null);
  let reviewing = $state(false);
  let panel: HTMLElement | undefined = $state();
  const back = { el: null as HTMLElement | null };

  const auth = (): Record<string, string> => (token ? { authorization: `Bearer ${token}` } : {});
  const remember = (on: boolean) => { try { if (on) sessionStorage.setItem(reopenKey, "1"); else sessionStorage.removeItem(reopenKey); } catch { /* the panel stays closed after the reload */ } };

  async function load(): Promise<void> {
    try {
      token = await getToken();
      const res = await fetch(`/api/artifacts/${ctl.id}/permissions`, { headers: auth() });
      answer = { ...(await readJson<object>(res)), id: ctl.id } as PermissionsView;
      rows = permissionRows(answer, (await ctl.host?.grantState("comments")) ?? null);
      error = null;
    } catch (e) {
      error = failureText("Could not load the permissions.", e);
    }
  }
  // What the controller's refetch (a permissions event, an approval from
  // another view or from this panel's Review) changed about approval: read
  // the answer again, so no row lingers as it was.
  const approvalKey = $derived(JSON.stringify([s.data?.artifact.capabilities_in_force ?? null, s.data?.artifact.pending_capabilities ?? null, s.data?.artifact.may_approve ?? null]));
  $effect(() => { void approvalKey; if (untrack(() => open)) void untrack(load); });
  async function show(at?: "policy") {
    back.el = document.activeElement as HTMLElement | null;
    open = true;
    void load();
    if (at === "policy") { await tick(); panel?.querySelector<HTMLElement>("button[name=content_security_policy]")?.focus(); }
  }

  // The Content Security Policy setting, where the daemon says it.
  const policy = $derived(s.data?.artifact.content_security_policy === undefined ? null : policyOf(s));
  const admin = $derived(s.data?.artifact.may_tag === true);
  let setting = $state(false);
  async function setPolicy(p: ContentSecurityPolicy): Promise<boolean> {
    if (setting) return false;
    setting = true;
    try {
      const r = await setContentSecurityPolicy(ctl.id, p);
      ctl.policyChanged(r.artifact?.content_security_policy ?? p);
      error = null;
      return true;
    } catch (e) {
      error = failureText("Could not change the Content Security Policy", e);
      return false;
    } finally {
      setting = false;
    }
  }
  function close() {
    open = false;
    reviewing = false;
    remember(false);
    if (back.el?.isConnected) back.el.focus?.();
  }
  onMount(() => {
    setPermissionsOpener(show);
    let again = false;
    try { again = sessionStorage.getItem(reopenKey) !== null; sessionStorage.removeItem(reopenKey); } catch { /* none */ }
    if (again) show();
    return () => setPermissionsOpener(null);
  });

  const ownerOnly = (r: PermissionRow) => typeof r.revoke === "object" && r.revoke !== null && !r.revoke.path.includes("/capability-approval/");
  const canRevoke = (r: PermissionRow) => r.revoke === "local" || (r.revoke !== null && (!ownerOnly(r) || token !== null));

  async function revoke(r: PermissionRow): Promise<void> {
    if (!r.revoke || working) return;
    working = r.key;
    const withdrawing = r.key.startsWith("approved:");
    try {
      if (r.revoke === "local") await ctl.host?.revoke(r.key);
      else {
        // The view reloads when what is served shrinks; the panel reopens then.
        if (withdrawing) remember(true);
        const res = await fetch(r.revoke.path, { method: r.revoke.method, headers: auth() });
        if (!res.ok && res.status !== 404) throw await readError(res);
        if (r.key === "sample") await ctl.host?.revoke("sample");
      }
      error = null;
    } catch (e) {
      if (withdrawing) remember(false);
      error = failureText("Could not take that back.", e);
    } finally {
      working = null;
    }
    await load();
  }

  function review() {
    ctl.showApproval();
    reviewing = true;
  }
  const modal = (el: HTMLElement) => {
    const undo = inertOutside(el);
    return () => undo();
  };
  const focus = (el: HTMLElement) => { el.focus(); };
  function keydown(e: KeyboardEvent) {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); close(); return; }
    if (panel) trapTab(e, panel);
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="perm-backdrop" {@attach modal} onclick={e => { if (e.target === e.currentTarget) close(); }}>
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div class="perm-panel" role="dialog" aria-modal="true" aria-labelledby="perm-title" tabindex="-1" bind:this={panel} onkeydown={keydown}>
      <h2 id="perm-title">Permissions for “{s.data?.artifact.title ?? ""}”</h2>
      {#if error}<p class="err" role="alert">{error}</p>{/if}
      {#if !answer && !error}<p class="muted">Loading…</p>{/if}
      {#if answer && !rows.length}<p class="muted">This page asks for nothing that needs your permission.</p>{/if}
      <ul>
        {#each rows as r (r.key)}
          <li data-permission={r.key}>
            <div class="what"><strong>{r.label}</strong><span>{r.state}</span></div>
            {#if r.state === "Waiting for approval"}
              <button type="button" onclick={review}>Review</button>
            {:else if canRevoke(r)}
              <button type="button" disabled={working !== null} onclick={() => { void revoke(r); }}>Revoke</button>
            {/if}
          </li>
        {/each}
      </ul>
      {#if policy && (admin || policy === "unrestricted")}
        <section class="policy" aria-labelledby="perm-policy">
          <h3 id="perm-policy">{CSP_HEADING}</h3>
          {#if admin}
            <ContentSecurityPolicySetting held={policy} busy={setting} set={setPolicy} note={SELF_NOTE} />
          {:else}
            <p class="muted" data-testid="csp-note">{CSP_NOTE}</p>
          {/if}
        </section>
      {/if}
      <div class="foot"><button type="button" {@attach focus} onclick={close}>Close</button></div>
    </div>
  </div>
  {#if reviewing && answer?.pending}
    <ApprovalDialog pending={answer.pending} title={s.data?.artifact.title ?? ""} busy={s.approving} owner={token !== null} onNotNow={() => ctl.notNow()} onApprove={() => { void ctl.approve().then(load); }} onClose={() => { reviewing = false; }} />
  {/if}
{/if}

<style>
  .perm-backdrop { position: fixed; inset: 0; z-index: 40; background: var(--scrim); display: grid; place-items: center; padding: var(--gutter); }
  .perm-panel { background: var(--raised); border: 1px solid var(--border-hover); border-radius: var(--radius); box-shadow: var(--elev-lg); width: min(520px, 100%); max-height: calc(100dvh - 32px); overflow: auto; padding: 18px 20px 20px; }
  .perm-panel h2 { margin: 0 0 12px; font-size: 17px; overflow-wrap: anywhere; }
  .perm-panel ul { list-style: none; margin: 0; padding: 0; display: grid; gap: 10px; font-size: 13px; }
  .perm-panel li { display: flex; gap: 12px; align-items: center; justify-content: space-between; }
  .perm-panel .what { display: grid; gap: 2px; min-width: 0; overflow-wrap: anywhere; }
  .perm-panel .what span, .perm-panel .muted { color: var(--muted); }
  .perm-panel .err { color: var(--danger, inherit); font-size: 13px; }
  .perm-panel .foot { margin-top: 16px; display: flex; justify-content: flex-end; }
  .perm-panel .policy { margin-top: 16px; padding-top: 14px; border-top: 1px solid var(--border); }
  .perm-panel h3 { margin: 0 0 8px; font-size: 13px; font-weight: 600; }
  .perm-panel .policy .muted { margin: 0; font-size: 13px; }
</style>
