// The DevTools page (manifest `devtools_page`): Chrome loads it in each
// DevTools window, where it only adds the Clax panel, and tells the panel's
// page when DevTools shows or hides it (the panel reads the page's settings
// only while shown). The panel is named "Clax", the extension's own name:
// its threads are Clax's, and the settings hook it reads is the Clax SDK's,
// on any site that uses it.
type ShownHook = { claxPanelShown?: (on: boolean) => void };

let panelWindow: (Window & ShownHook) | null = null;
chrome.devtools.panels.create("Clax", "icons/32.png", "devtools-panel.html", panel => {
  panel.onShown.addListener(w => { panelWindow = w as Window & ShownHook; panelWindow.claxPanelShown?.(true); });
  panel.onHidden.addListener(() => { panelWindow?.claxPanelShown?.(false); });
});
