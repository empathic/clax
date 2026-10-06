// The composer page's entry (spec 2026-10-05 L7, §9.4): the overlay frames
// it at a pick, its pick ID in the URL's fragment; it talks only to the
// worker, over a port named for the pick.
import { mount } from "svelte";
import "../../../shell/src/theme.css";
import { PICK_ID } from "../messages";
import ComposerFrame from "./ComposerFrame.svelte";

const pickId = location.hash.slice(1);
if (PICK_ID.test(pickId)) {
  const port = chrome.runtime.connect({ name: `composer:${pickId}` });
  mount(ComposerFrame, { target: document.getElementById("app")!, props: { port, pickId } });
}
