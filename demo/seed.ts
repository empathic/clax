// Seeds a fresh Clax in hosted mode for the demo (lib/seed.ts), through the
// routes a browser uses, and prints the links to open:
//
//   node demo/seed.ts --url <base> --secret <setup secret> [--owner Rae] [--editor Mia] [--invitee Jo] [--agents 3] [--mcp-path /mcp/json] [--json]
//
// The instance must have no one yet: setup works once, so run seed instead
// of opening /setup. `--json` prints what was made as JSON instead.

import { parseArgs } from "node:util";
import { NATIVE_MCP_PATH } from "./lib/http.ts";
import { describe, seed } from "./lib/seed.ts";

const { values } = parseArgs({
  options: {
    url: { type: "string" },
    secret: { type: "string" },
    owner: { type: "string", default: "Rae" },
    editor: { type: "string", default: "Mia" },
    invitee: { type: "string", default: "Jo" },
    agents: { type: "string", default: "3" },
    "mcp-path": { type: "string", default: NATIVE_MCP_PATH },
    json: { type: "boolean", default: false },
    help: { type: "boolean", short: "h" },
  },
});

if (values.help === true || values.url === undefined || values.secret === undefined) {
  console.error("usage: node demo/seed.ts --url <base URL> --secret <setup secret> [--owner Rae] [--editor Mia] [--invitee Jo] [--agents 3] [--mcp-path /mcp/json] [--json]");
  process.exit(values.help === true ? 0 : 2);
}

try {
  const s = await seed({ url: values.url, secret: values.secret, owner: values.owner, editor: values.editor, invitee: values.invitee, agents: Number(values.agents), mcpPath: values["mcp-path"] });
  if (values.json) {
    const { session: _o, ...owner } = s.owner;
    const { session: _e, ...editor } = s.editor;
    console.log(JSON.stringify({ ...s, owner, editor }, null, 2));
  } else {
    console.log(`${describe(s)}
Run the demo agents with it:
  node demo/agents.ts --url ${s.base} --mcp-path ${values["mcp-path"]} --token ${s.token.token} --id ${s.brief.id} --agents ${s.agents}`);
  }
} catch (e) {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
}
