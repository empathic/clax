// What the artifact view starts after its first paint about the people on
// it, in one chunk: presence reports (`presence-reporter.ts`) and the live
// layers (`live-view.ts`), so the entry names one lazy module for both.
export { PresenceReporter } from "./presence-reporter";
export { LiveView } from "./live-view";
