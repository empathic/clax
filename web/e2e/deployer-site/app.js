/* global Clax */
// The deployer page's own script: it starts the Clax overlay on its queue,
// and gets each reporter token from its own backend (`POST /clax-token`),
// which knows the signed-in user by their cookie.
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
});
