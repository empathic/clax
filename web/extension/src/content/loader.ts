// The content script for each origin Clax is on (spec 2026-10-05 §12):
// under 2 KiB. It tells the worker the page's URL on load and on every
// same-document navigation; the worker injects the overlay when the page
// has threads or the person turned Clax on in this tab.
import { startLoader } from "./loader-app";

startLoader({ win: window, runtime: chrome.runtime as never });
