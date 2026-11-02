// The side panel's entry (spec 2026-10-05 §6.4): the shell's theme, and the
// panel over a link to the worker for this window, which hears whether the
// panel holds a draft.
import { mount } from "svelte";
import "../../../shell/src/theme.css";
import { watchDrafts } from "./draft";
import { PanelLink } from "./link.svelte";
import Panel from "./Panel.svelte";

void chrome.windows.getCurrent().then(win => {
  const link = new PanelLink(win.id!);
  mount(Panel, { target: document.getElementById("app")!, props: { link } });
  watchDrafts({ doc: document }, on => link.draft(on));
});
