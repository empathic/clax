import { mount } from "svelte";
import Gallery from "./ui/Gallery.svelte";
import { installTransport } from "./transport";

// In the Clax app the API goes over the app's channel: before anything
// else asks for it.
installTransport();

const target = document.getElementById("app")!;
// A hosted daemon's sign-in pages (`/setup`, `/device`, `/join/<code>`, `/signin[/<code>]`)
// load their own chunk; every other path is the gallery's.
if (/^\/(setup$|device$|join\/|signin(\/|$))/.test(location.pathname)) void import("./hosted/SignIn.svelte").then(m => mount(m.default, { target }));
else mount(Gallery, { target });
