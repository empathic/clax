import { mount } from "svelte";
import { bootFeedback } from "./feedback/state.svelte";
import Gallery from "./ui/Gallery.svelte";

const target = document.getElementById("app")!;
// A hosted daemon's sign-in pages (`/setup`, `/device`, `/join/<code>`, `/signin[/<code>]`)
// load their own chunk; every other path is the gallery's.
if (/^\/(setup$|device$|join\/|signin(\/|$))/.test(location.pathname)) void import("./hosted/SignIn.svelte").then(m => mount(m.default, { target }));
else {
  // Asked first, so the feedback overlay's recorder starts as early as it can.
  bootFeedback();
  mount(Gallery, { target });
}
