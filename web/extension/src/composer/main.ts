// The composer page's entry (spec 2026-10-05 L7, §9.4): the overlay frames
// it at a pick, its pick ID in the URL's fragment; it talks only to the
// worker, over a port named for the pick.
import { mount } from "svelte";
import "../../../shell/src/theme.css";
import { PICK_ID } from "../messages";
import ComposerFrame from "./ComposerFrame.svelte";

/** Connects once this page has loaded: the worker then tells the overlay
 * to show the frame, so the frame shown is one whose own load has run, and
 * any later load of it is a navigation the overlay closes it for. */
function start(): void {
  const pickId = location.hash.slice(1);
  if (!PICK_ID.test(pickId)) return;
  const port = chrome.runtime.connect({ name: `composer:${pickId}` });
  mount(ComposerFrame, { target: document.getElementById("app")!, props: { port, pickId } });
}
if (document.readyState === "complete") start();
else addEventListener("load", start, { once: true });
