// The SDK package's main entry (`@empathic/clax-sdk`, scripts/build-sdk.mjs):
// what a deployer's page needs to file reports into a queue and read the
// replies, safe to ship to a browser. The overlay is `./overlay`; the
// backend's service client, which holds a queue's service grant, is
// `./service` and never ships to a page.
export { ClaxError } from "./client";
export { ReporterClient, type NewReport, type ReporterAuth, type ReporterOptions } from "./reporter";
export type * from "./types";
