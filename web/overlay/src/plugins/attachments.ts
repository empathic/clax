// Attachment plugins: what a report may carry beyond its note, each behind
// its own box and each with its line in "What will be sent". They land on
// the report's existing wire fields: log lines as `diagnostics`, the app's
// version as `context.release`, the browser as `context.environment`.
import { definePlugin, PLUGIN_API_VERSION, type SentRow } from "../../../sdk/src/plugin";
import { cut, scrub } from "../../../sdk/src/scrub";
import type { DiagLine, Environment, Release } from "../../../sdk/src/types";

/** A line of the host's log, as Changeling's AppLog keeps it. */
export interface LogLine {
  at: string;
  level: "debug" | "info" | "warn" | "error" | "request";
  text: string;
}

/** The most log lines a report carries (Clax keeps 200 diagnostics). */
export const MAX_LOG_LINES = 200;
/** The longest log line, in characters. */
export const MAX_LOG_TEXT = 300;

/** `l` as a diagnostics line: scrubbed (the SDK's rules, which the server runs again), one line, cut. */
export function diagOf(l: LogLine): DiagLine {
  const first = String(l.text ?? "").split(/\r?\n/)[0];
  const at = typeof l.at === "string" && l.at.length <= 64 ? l.at : new Date().toISOString();
  if (l.level === "request") return { at, level: "error", kind: "request", text: cut(scrub(first), MAX_LOG_TEXT) };
  const level = (["debug", "info", "warn", "error"] as const).includes(l.level as never) ? (l.level as DiagLine["level"]) : "info";
  return { at, level, kind: "console", text: cut(scrub(first), MAX_LOG_TEXT) };
}

const row = (r: Omit<SentRow, "optIn">): SentRow => ({ optIn: true, ...r });

/** The app's recent log lines, from `source` (the host's log; read when the
 * report starts and again at Send). Off unless `default` is true. */
export const logs = (opts: { source: () => readonly LogLine[]; default?: boolean; label?: string }) => {
  if (typeof opts?.source !== "function") throw new TypeError("logs(): source must be a function giving the log lines");
  const label = opts.label ?? "The app's recent log lines";
  return definePlugin({
    name: "logs", version: "1.0.0", apiVersion: PLUGIN_API_VERSION, capabilities: [],
    contributes: { attachments: [{
      id: "logs", label, default: opts.default ?? false,
      collect() {
        const lines = opts.source().slice(-MAX_LOG_LINES).map(diagOf);
        return {
          data: { diagnostics: lines },
          row: row({ id: "logs", label: `${label} (${lines.length})`, preview: lines.map(l => `${l.level} ${l.text}`).join("\n") || "Nothing logged yet.", default: opts.default ?? false, privacyNote: "One line each, with tokens, cookies and anything spanning lines left out." }),
        };
      },
    }] },
  });
};

/** The app's version (`release`, or a function giving it when the report starts). On by default. */
export const appVersion = (opts: { release: Release | (() => Release); default?: boolean }) => definePlugin({
  name: "app-version", version: "1.0.0", apiVersion: PLUGIN_API_VERSION, capabilities: [],
  contributes: { attachments: [{
    id: "version", label: "The app's version", default: opts.default ?? true,
    collect() {
      const r = typeof opts.release === "function" ? opts.release() : opts.release;
      const release: Release = {};
      for (const k of ["id", "version", "commit", "channel"] as const) if (typeof r?.[k] === "string" && r[k]) release[k] = cut(r[k]!, 128);
      const shown = [release.version, release.commit && `(${release.commit.slice(0, 9)})`, release.channel].filter(Boolean).join(" ") || "unknown";
      return { data: { release }, row: row({ id: "version", label: "Version", preview: shown, default: opts.default ?? true }) };
    },
  }] },
});

/** The browser: its user agent, platform, language and time zone. On by
 * default; unticked, the report's context leaves them out. */
export const browser = (opts: { default?: boolean } = {}) => definePlugin({
  name: "browser", version: "1.0.0", apiVersion: PLUGIN_API_VERSION, capabilities: [],
  contributes: { attachments: [{
    id: "browser", label: "Your browser", default: opts.default ?? true,
    collect({ doc }) {
      const nav = doc.defaultView?.navigator;
      const environment: Partial<Environment> = {};
      if (nav?.userAgent) environment.user_agent = nav.userAgent;
      const platform = (nav as Navigator & { userAgentData?: { platform?: string } })?.userAgentData?.platform ?? nav?.platform;
      if (platform) environment.platform = platform;
      if (nav?.language) environment.locale = nav.language;
      try { environment.time_zone = Intl.DateTimeFormat().resolvedOptions().timeZone; } catch { /* none */ }
      return { data: { environment }, row: row({ id: "browser", label: "Browser", preview: [environment.user_agent, environment.locale, environment.time_zone].filter(Boolean).join(" · "), default: opts.default ?? true }) };
    },
  }] },
});

/** The environment keys the browser attachment owns: left out when it is unticked. */
export const BROWSER_KEYS = ["user_agent", "platform", "locale", "time_zone"] as const;
