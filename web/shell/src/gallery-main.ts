import "./owner-proof-install";
import { mount } from "svelte";
import { bootFeedback } from "./feedback/boot";
import Gallery from "./ui/Gallery.svelte";
import { registerServiceWorker } from "./pwa";

const target = document.getElementById("app")!;
// A hosted daemon's sign-in pages (`/setup`, `/device`, `/join/<code>`, `/signin[/<code>]`)
// load their own chunk, as does its landing page (`/start/`); every other path is the gallery's.
if (/^\/(setup$|device$|join\/|signin(\/|$))/.test(location.pathname)) void import("./hosted/SignIn.svelte").then(m => mount(m.default, { target }));
else if (location.pathname === "/start/") void import("./hosted/Landing.svelte").then(m => mount(m.default, { target }));
// A reporter's view of its queue (`/reporter/<qid>[/<tid>]`) is its own chunk too: a reporter has no session of the gallery.
else if (/^\/reporter\//.test(location.pathname)) void import("./queues/ReporterRoute.svelte").then(m => mount(m.default, { target }));
else {
  // Reporting a problem with Clax (`feedback/`), asked first so the
  // overlay's recorder starts as early as it can, whatever the gallery's
  // own requests answer.
  bootFeedback();
  mount(Gallery, { target });
}
registerServiceWorker();
