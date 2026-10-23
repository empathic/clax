<svelte:options css="injected" />

<script lang="ts">
  // `/admin` (spec 2026-10-21-claxbot §5, §11.5), for admins: People & flags,
  // Provider, Pricing and Usage, one at a time by `?tab=`, with links to the
  // audit log and, on the localhost daemon, its settings. Anyone else is told
  // the page is for admins and fetches nothing more. The owner alone changes
  // roles (the People tab shows them read-only to other admins).
  import { onMount, type Component } from "svelte";
  import type { Role } from "./api";
  import { hosted, isAdmin, myRole } from "./me";
  import BotPage from "./BotPage.svelte";

  let { now }: { now?: () => Date } = $props();
  type Tab = "people" | "provider" | "pricing" | "usage";
  const TABS: { id: Tab; label: string }[] = [{ id: "people", label: "People & flags" }, { id: "provider", label: "Provider" }, { id: "pricing", label: "Pricing" }, { id: "usage", label: "Usage" }];
  const asked = new URLSearchParams(location.search).get("tab");
  const tab: Tab = TABS.some(t => t.id === asked) ? asked as Tab : "people";
  let role = $state<Role | null>(null);
  let local = $state(false);
  let Body = $state<Component<{ role: Role; now?: () => Date }> | null>(null);
  const LOAD: Record<Tab, () => Promise<{ default: unknown }>> = {
    people: () => import("./AdminPeople.svelte"), provider: () => import("./AdminProvider.svelte"), pricing: () => import("./AdminPricing.svelte"), usage: () => import("./AdminUsage.svelte"),
  };
  const tabs = $derived([
    ...TABS.map(t => ({ id: t.id as string, label: t.label, href: `/admin?tab=${t.id}` })),
    { id: "audit", label: "Audit", href: "/audit" },
    ...(local ? [{ id: "settings", label: "Settings", href: "/settings" }] : []),
  ]);
  onMount(() => {
    document.title = "Admin · Clax";
    void myRole().then(r => {
      role = r;
      if (isAdmin(r)) void LOAD[tab]().then(m => { Body = m.default as Component<{ role: Role; now?: () => Date }>; }, () => {});
    });
    void hosted().then(h => { local = !h; });
  });
</script>

<BotPage title="Admin" lede="Who may use ClaxBot and with how many credits, the provider it calls, what a credit costs, and what this week cost. The owner is always an admin and alone makes others admins." {tabs} current={tab}>
  {#if role !== null && !isAdmin(role)}
    <p class="note">Admin is for the owner and the admins they name. Your own ClaxBot usage is under <a href="/me">You</a>.</p>
  {:else if role && Body}
    <Body {role} {now} />
  {:else}
    <p class="note" role="status">Loading…</p>
  {/if}
</BotPage>
