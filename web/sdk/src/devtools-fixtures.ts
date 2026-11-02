// The app dev settings fixtures the SDK's and the panel's tests share:
// Changeling's "Feedback style" today (its first client, the options and
// "Needs from the Clax SDK" notes of changelist web/src/lib/feedback/
// presentation.ts), and the separate axes the feedback-plugins BRIEF
// Addendum 2 asks for, with values from the Clax config and the site.
import type { SectionInit } from "./devtools";

export const changelingStyle: SectionInit = {
  id: "feedback",
  title: "Feedback",
  description: "How Changeling presents feedback. Development only; changeling.io always uses its own Send feedback.",
  fields: [{
    id: "style", label: "Feedback style", kind: "radio", applies: "live",
    base: { value: "native", source: "site" },
    options: [
      { value: "native", label: "Changeling’s Send feedback (prod)", description: "The Feedback button and F open Changeling’s own dialog; it files into the Clax queue" },
      { value: "button", label: "Header button → Clax overlay (clax-clippy)", description: "Changeling’s Feedback button and F start a report in the Clax overlay" },
      { value: "floating", label: "Floating launcher", description: "The overlay’s own Report and “Your reports” buttons in the bottom right corner" },
      { value: "off", label: "Clax off", description: "No overlay: the Feedback button and F are hidden" },
      { value: "drawer", label: "Side drawer", description: "The report form in a panel along the right edge", disabled: true, needs: '`presentation: "drawer"`' },
      { value: "sheet", label: "Bottom sheet", description: "The report form rising from the bottom, at phone width", disabled: true, needs: '`presentation: "sheet"`' },
      { value: "pins", label: "Inline pins", description: "Reports pinned on the element or line they are about", disabled: true, needs: "pins on a host page outside a Clax artifact" },
      { value: "screenshot", label: "Screenshot, then annotate", description: "Starts by capturing the page, then draw on it", disabled: true, needs: '#38 `report({ mode: "screen" })` on the instance' },
      { value: "picker", label: "Element picker first", description: "Starts by picking the element, by pointer or keyboard", disabled: true, needs: '#38 `report({ mode: "element" })` and keyboard-pick.ts on the instance' },
      { value: "palette", label: "Command palette entry", description: "“Send feedback” in Changeling’s finder", disabled: true, needs: "a hotkeys / actions extension point (`handle.actions()`)" },
    ],
  }],
};

export const feedbackAxes: SectionInit = {
  id: "clax",
  title: "Clax feedback",
  description: "From this site's Clax container; your changes are kept in this browser only.",
  fields: [
    { id: "enabled", label: "Clax on", kind: "toggle", applies: "reload", base: { value: true, source: "clax" } },
    { id: "entry-header", label: "Header button", kind: "toggle", applies: "live", base: { value: true, source: "site" } },
    { id: "entry-key", label: "F key", kind: "toggle", applies: "live", base: { value: true, source: "clax" } },
    {
      id: "presentation", label: "Presentation", kind: "radio", applies: "reload", base: { value: "dialog", source: "clax" },
      options: [
        { value: "dialog", label: "Dialog" }, { value: "drawer", label: "Drawer" }, { value: "sheet", label: "Sheet" },
        { value: "window", label: "Separate window" }, { value: "extension", label: "Clax extension", description: "Falls back to Dialog without the extension" },
      ],
    },
    { id: "attach-logs", label: "Attach logs by default", kind: "toggle", applies: "live", base: { value: false, source: "site" }, locked: true },
    { id: "note", label: "Note placeholder", kind: "text", applies: "live", maxLength: 80, base: { value: "What happened?", source: "site" } },
  ],
};
