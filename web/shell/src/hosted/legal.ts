// A hosted Clax's Terms of Service and Privacy Policy (`me.legal`,
// crates/clax-server/src/hosted.rs). A signed-in person who has not agreed
// to their current version is shown the prompt (LegalPrompt.svelte), once
// per page load, until they agree. The gallery reads `GET /api/hosted/me`
// itself and calls `ask`; the artifact view's more menu asks through
// `hostedMe`.
import { mount, unmount } from "svelte";
import * as api from "./api";

let asked: Promise<api.HostedMe | null> | null = null;
let shown = false;

/** The browser's hosted sign-in, asked once per page load, or null on a
 * local daemon (or when asking fails); asks for agreement as `ask` does. */
export function hostedMe(): Promise<api.HostedMe | null> {
  asked ??= api.me().then(me => {
    if (me?.legal) ask(me.legal);
    return me;
  }, () => null);
  return asked;
}

/** Shows the prompt when this browser's person has not agreed to `legal`,
 * once per page load; resolves once it is shown or not needed. */
export async function ask(legal: api.HostedLegal): Promise<void> {
  if (legal.agreed !== false || shown) return;
  shown = true;
  const m = await import("./LegalPrompt.svelte");
  const prompt = mount(m.default, { target: document.body, props: { legal, done: () => void unmount(prompt) } });
}
