// The design gallery (design/README.md, "The gallery"): every shell
// primitive and the overlay's panels in their variants, on the shell's own
// generated tokens, in the theme `?theme=light|dark` chooses (else the
// system's). A development page: no production build includes it
// (vite.design-gallery.config.ts builds it alone).
import { mount } from "svelte";
import "../shell/src/theme.css";
import Gallery from "./Gallery.svelte";

const theme = new URLSearchParams(location.search).get("theme");
if (theme === "light" || theme === "dark") document.documentElement.dataset.theme = theme;

mount(Gallery, { target: document.getElementById("gallery")!, props: { theme: theme === "light" || theme === "dark" ? theme : null } });
