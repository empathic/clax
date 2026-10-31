// @empathic/clax-client/stream: the live stream. One `Hub` holds the
// daemon's event stream for every tab of a client and fans its messages
// out (`HubMsg` from the hub, `TabMsg` from a tab). Free of Svelte and the
// DOM. Part of web/shell (AGPL-3.0-only).
export { Hub, type HubMsg, type TabMsg } from "../src/stream-hub";
