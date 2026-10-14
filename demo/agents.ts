// Several scripted agents writing one artifact at once, over MCP with an
// agent token (lib/agents.ts):
//
//   node demo/agents.ts --url <base> --token <cxa_…> [--mcp-path /mcp/json]
//                       [--agents 3] [--pace 2500] [--listen 600] [--id <artifact ID>]
//
// The agents (one MCP session each under the token) draft their own
// sections of a product brief in rounds, conflict and merge, say what they
// are doing with `working`, then wait for comments sent to them and answer
// each with a new version, a reply and a resolve. The content is fixed
// text; no model is called. Ctrl-C stops them: they publish nothing more and
// their sessions end. `--mcp-path` is `/mcp/json` on the native daemon and
// `/mcp` on the Worker.

import { parseArgs } from "node:util";
import { runDemo } from "./lib/agents.ts";
import { NATIVE_MCP_PATH } from "./lib/http.ts";

const { values } = parseArgs({
  options: {
    url: { type: "string" },
    token: { type: "string" },
    "mcp-path": { type: "string", default: NATIVE_MCP_PATH },
    agents: { type: "string", default: "3" },
    pace: { type: "string", default: "2500" },
    listen: { type: "string", default: "600" },
    id: { type: "string" },
    help: { type: "boolean", short: "h" },
  },
});

const USAGE = "usage: node demo/agents.ts --url <base URL> --token <cxa_…> [--mcp-path /mcp/json] [--agents 3] [--pace ms] [--listen s] [--id <artifact ID>]";

if (values.help === true || values.url === undefined || values.token === undefined) {
  console.error(USAGE);
  process.exit(values.help === true ? 0 : 2);
}

const controller = new AbortController();
process.on("SIGINT", () => {
  if (controller.signal.aborted) process.exit(130);
  console.log("stopping: ending the agents' sessions (Ctrl-C again to quit at once)");
  controller.abort();
});
process.on("SIGTERM", () => controller.abort());

const started = Date.now();
const stamp = () => `[${((Date.now() - started) / 1000).toFixed(1).padStart(5)}s]`;

try {
  const r = await runDemo({
    url: values.url,
    token: values.token,
    mcpPath: values["mcp-path"],
    agents: Number(values.agents),
    paceMs: Number(values.pace),
    listenS: Number(values.listen),
    artifactId: values.id,
    signal: controller.signal,
    log: (line) => console.log(`${stamp()} ${line}`),
    onArtifact: (a) => console.log(`${stamp()} open ${a.url}`),
  });
  console.log(
    `${stamp()} done: ${r.versions.length} versions, ${r.conflicts.length} conflicts merged, ${r.answered.length} comments answered, ${r.held.length} held, on ${r.url}`,
  );
} catch (e) {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
}
