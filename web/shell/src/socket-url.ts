/** The WebSocket URL of `path` on `origin` (`https://host`), or null where
 * the shell opens no WebSocket: in the Clax app (`app`: `onHost()` where a
 * window is), whose scheme handlers never see a WebSocket handshake and
 * whose core refuses `/live` and `/room` (spec 2026-10-19-in-process-core-design
 * §4: no app platform uses `ws:`), and on any origin but `https:` (`wss:`) or
 * a browser's plain-`http:` daemon (`ws:`), so no scheme is guessed. */
export function socketUrl(origin: string, path: string, app: boolean): string | null {
  const m = /^(https?):(\/\/[^/]+)$/.exec(origin);
  return app || !m ? null : `${m[1] === "https" ? "wss" : "ws"}:${m[2]}${path}`;
}
