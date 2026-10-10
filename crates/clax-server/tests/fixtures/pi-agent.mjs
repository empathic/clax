// Drives the Pi extension (plugins/pi/src/clax.ts) as Pi does, against a
// running daemon, and prints everything it hands Pi's model or person: each
// tool's result, the `/clax` command's notices, and the follow-up messages
// its feedback loop sends. Used by clax-server's canary sweep.
//
// Usage, from plugins/pi (whose jiti loads the TypeScript, as Pi does):
//   node --import jiti/register pi-agent.mjs <plugins/pi/src/clax.ts> <CLAX_HOME> <Pi session ID> <artifact ID>
const [ext, home, sessionId, aid] = process.argv.slice(2);
const mod = await import(ext);
const claxExtension = mod.claxExtension ?? mod.default.claxExtension;
const handlers = new Map();
const tools = new Map();
const commands = new Map();
const said = [];
const api = {
  on: (event, h) => handlers.set(event, [...(handlers.get(event) ?? []), h]),
  registerTool: t => tools.set(t.name, t),
  registerCommand: (name, o) => commands.set(name, o),
  sendUserMessage: content => said.push(`sent: ${JSON.stringify(content)}`),
};
claxExtension({ home, env: { ...process.env } })(api);
const ctx = {
  cwd: process.cwd(),
  hasUI: false,
  sessionManager: { getSessionId: () => sessionId },
  ui: { notify: (message, type) => said.push(`notify(${type ?? ""}): ${message}`) },
};
const emit = async (event, payload) => {
  for (const h of handlers.get(event) ?? []) await h(payload, ctx);
};
await emit("session_start", { type: "session_start", reason: "startup" });
const calls = [
  ["clax_status", {}],
  ["clax_list", {}],
  ["clax_read", { url_or_id: aid }],
  ["clax_read", { url_or_id: aid, version: 1, path: "MIACANARYfilename.html" }],
  ["clax_comments_read", { url_or_id: aid, include_resolved: true }],
  ["clax_db_list", { url_or_id: aid, collection: "notes" }],
  ["clax_wait_for_feedback", { timeout_s: 1 }],
];
for (const [name, params] of calls) {
  const t = tools.get(name);
  if (!t) {
    said.push(`${name}: no such tool`);
    continue;
  }
  try {
    let r = await t.execute("call-1", params, undefined, undefined, ctx);
    let content = r.content;
    for (const h of handlers.get("tool_result") ?? []) {
      const patch = await h({ type: "tool_result", toolName: name, toolCallId: "call-1", input: params, content, isError: false }, ctx);
      if (patch?.content) content = patch.content;
    }
    said.push(`${name}: ${JSON.stringify(content)}`);
  } catch (e) {
    said.push(`${name}: error: ${e instanceof Error ? e.message : String(e)}`);
  }
}
for (const args of ["list", "status", `open ${aid}`]) {
  try {
    await commands.get("clax").handler(args, ctx);
  } catch (e) {
    said.push(`/clax ${args}: error: ${String(e)}`);
  }
}
await emit("session_shutdown", { type: "session_shutdown", reason: "quit" });
process.stdout.write(said.join("\n") + "\n");
process.exit(0);
