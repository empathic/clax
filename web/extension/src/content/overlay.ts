// The overlay content script, injected by the worker into a tab's top frame
// (spec 2026-10-05 §6.4). Injected twice into one document, it starts once.
import { startOnce } from "./overlay-app";

startOnce({ doc: document, runtime: chrome.runtime as never });
