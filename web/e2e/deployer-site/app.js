/* global Clax */
// The deployer page's own script: it starts the Clax overlay on its queue,
// and gets each reporter token from its own backend (`POST /clax-token`),
// which knows the signed-in user by their cookie. Each capture carries the
// site's release and flags (a fixed release, as a build would stamp it),
// and "Include recent errors" starts ticked.
const { instance, queue, nonce } = document.documentElement.dataset;
Clax.overlay({
  instance,
  queue,
  // The page's policy refuses inline style but for this nonce.
  nonce,
  auth: async publicKey => {
    const r = await fetch("/clax-token", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ public_key: publicKey }) });
    if (!r.ok) throw new Error(`the site gave no reporter token: ${r.status} ${await r.text()}`);
    return r.json();
  },
  context: () => ({
    release: { version: "2.3.0", commit: "e2e0000", channel: "staging" },
    flags: { single_day_requests: true, approvals: "v2" },
    app: { repo: "example/leave" },
  }),
  diagnostics: { default: true },
});

// Approving a one-day request fails: the page logs the error the overlay records.
for (const button of document.querySelectorAll(".request .approve")) {
  button.addEventListener("click", () => {
    const request = button.closest(".request");
    if (request.id === "req-2") console.error(`approve ${request.id}: the request has no end date`);
  });
}
