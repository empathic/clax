import "./owner-proof-install";
import { mount } from "svelte";
import { bootFeedback } from "./feedback/boot";
import Gallery from "./ui/Gallery.svelte";
import { registerServiceWorker } from "./pwa";
import { answered, readGalleryBoot } from "./view/gallery-boot";
import { replaceFrame } from "./view/gallery-frame";
import { LANDING_PATH, SIGN_IN_PATH } from "./view/gallery-start";

const target = document.getElementById("app")!;
// A hosted daemon's sign-in pages (`/setup`, `/device`, `/join/<code>`, `/signin[/<code>]`)
// load their own chunk, as does its landing page (`/start/`); every other path is the gallery's.
// The page's static frame (`view/gallery-frame.ts`) is the gallery's: they show none of it
// (`gallery.css` hides it by the page's kind until `bare` removes it and the kind with it,
// so the page's own header shows).
const bare = () => { target.replaceChildren(); delete document.documentElement.dataset.page; };
if (SIGN_IN_PATH.test(location.pathname)) { bare(); void import("./hosted/SignIn.svelte").then(m => mount(m.default, { target })); }
else if (location.pathname === LANDING_PATH) { bare(); void import("./hosted/Landing.svelte").then(m => mount(m.default, { target })); }
// A reporter's view of its queue (`/reporter/<qid>[/<tid>]`) is its own chunk too: a reporter has no session of the gallery.
else if (/^\/reporter\//.test(location.pathname)) { bare(); void import("./queues/ReporterRoute.svelte").then(m => mount(m.default, { target })); }
else {
  const boot = readGalleryBoot();
  // Reporting a problem with Clax (`feedback/`), asked first so the
  // overlay's recorder starts as early as it can, whatever the gallery's
  // own requests answer; the block's answer, when it has one, asks nothing.
  bootFeedback(undefined, boot && answered(boot, "feedback") ? { body: boot.feedback } : undefined);
  // The gallery takes the static frame's place in this task: the first
  // frame painted with it is the frame's layout.
  replaceFrame(target, anchor => { mount(Gallery, { target, anchor, props: { boot } }); });
}
registerServiceWorker();
