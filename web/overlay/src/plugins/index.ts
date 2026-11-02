// Clax's first-party plugins (`@empathic/clax-sdk/plugins`): every feedback
// feature of prod Changeling's Send-feedback dialog, as plugins of the SDK's
// contract (sdk/src/plugin.ts). A host passes the ones it wants to
// `overlay({ plugins })`, or names them in its config and the loader
// (`install`, overlay/src/loader.ts) brings in only those.
export { annotate, ANNOTATE_TOOLS } from "./annotate";
export { appVersion, browser, BROWSER_KEYS, diagOf, logs, type LogLine } from "./attachments";
export { displayCapture, domCapture, type CaptureRequest } from "./capture";
export { codeBlur, codeRegions, CODE_SELECTOR, type BlurDeps } from "./code-blur";
export { areaPicker, codeLine, elementPicker, locateCode, pagePicker, type CodeLocation } from "./pickers";
export { EXTENSION_ATTR, EXTENSION_EVENT, extensionPresent, extensionPresentation, handToExtension, presentations, theme, windowPresentation } from "./presentations";
export { definePlugin, PLUGIN_API_VERSION, PluginHost, type ClaxPlugin } from "../../../sdk/src/plugin";

export { classic } from "./classic";
