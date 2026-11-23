import "./owner-proof-install";
import { mount } from "svelte";
import { bootFeedback } from "./feedback/boot";
import Gallery from "./ui/Gallery.svelte";
import { answered, readGalleryBoot } from "./view/gallery-boot";
import { replaceFrame } from "./view/gallery-frame";
import { LANDING_PATH, SIGN_IN_PATH } from "./view/gallery-start";

const target = document.getElementById("app")!;
// A hosted daemon's sign-in pages (`/setup`, `/device`, `/join/<code>`, `/signin[/<code>]`)
// load their own chunk, as does its landing page (`/start/`); every other path is the gallery's.
// The entry's static frame (`view/gallery-frame.ts`) shows each its own
// frame until its chunk is in, and the page takes the frame's place in the
// task its chunk arrives in: the frame painted last is the page's layout.
// Their block's reader (`hosted/signin-boot.ts`) is in their chunks, not
// the gallery's entry.
if (SIGN_IN_PATH.test(location.pathname)) {
  void Promise.all([import("./hosted/SignIn.svelte"), import("./hosted/signin-boot")]).then(([m, b]) => { const boot = b.readSignInBoot(); target.replaceChildren(); mount(m.default, { target, props: { boot } }); });
} else if (location.pathname === LANDING_PATH) {
  void Promise.all([import("./hosted/Landing.svelte"), import("./hosted/signin-boot")]).then(([m, b]) => {
    const boot = b.readSignInBoot();
    const signedIn = b.statusOf(boot, "hosted") === 200 ? !!boot?.hosted?.signed_in : null;
    // The quickstart's frame stays where the static frame put it (moving
    // an iframe loads it again); the landing's header takes the frame's.
    const stage = landingStage();
    const box = stage.parentElement!;
    for (const n of [...target.childNodes]) if (n !== box) n.remove();
    box.querySelector(":scope > .gbar")?.remove();
    mount(m.default, { target: box, anchor: stage, props: { signedIn } });
  });
} else {
  const boot = readGalleryBoot();
  // Reporting a problem with Clax (`feedback/`), asked first so the
  // overlay's recorder starts as early as it can, whatever the gallery's
  // own requests answer; the block's answer, when it has one, asks nothing.
  bootFeedback(undefined, boot && answered(boot, "feedback") ? { body: boot.feedback } : undefined);
  // The gallery takes the static frame's place in this task: the first
  // frame painted with it is the frame's layout. A hosted Clax's Terms
  // that this person has not agreed to are asked in the same task (the
  // prompt's chunk preloaded beside the entry's by the inline `clax-page`
  // script), so the prompt is there at the gallery's first render.
  const legal = boot && answered(boot, "hosted") && boot.hosted?.legal?.agreed === false ? boot.hosted.legal : null;
  const show = (ask?: () => void) => {
    replaceFrame(target, anchor => { mount(Gallery, { target, anchor, props: { boot } }); });
    ask?.();
  };
  if (legal) void Promise.all([import("./hosted/legal"), import("./hosted/LegalPrompt.svelte")]).then(([m, p]) => show(() => m.askWith(legal, p.default)), () => show());
  else show();
}

/** The landing page's quickstart frame: the static frame's, or one made
 * as the inline `clax-framed` script makes it where that is missing. */
function landingStage(): HTMLIFrameElement {
  const found = target.querySelector<HTMLIFrameElement>(":scope > .aframe > iframe.lframe");
  if (found) return found;
  const box = document.createElement("div");
  box.className = "aframe";
  const f = document.createElement("iframe");
  f.className = "lframe";
  f.src = "/start/index.html";
  f.title = "Start here: Clax in five minutes";
  box.append(f);
  target.replaceChildren(box);
  return f;
}
