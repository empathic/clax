// The SDK's TypeScript types, generated from the API contract
// (web/sdk/openapi.json, which crates/clax-server generates from the routes:
// spec 2026-10-27-clax-sdk-design D2, §4.1) into web/sdk/src/contract.gen.ts.
//
//   node scripts/contract-types.mjs           write contract.gen.ts
//   node scripts/contract-types.mjs --check   fail unless it is up to date
//
// Types only: every schema of the contract's components as an interface or
// a type alias, and `Operations`, each operation's method, path, bodies,
// error codes and §4.1 extensions. No dependency: the contract's schemas
// are the JSON Schema subset schemars writes.

import { readFileSync, realpathSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const web = join(dirname(fileURLToPath(import.meta.url)), "..");
export const CONTRACT = join(web, "sdk", "openapi.json");
export const GENERATED = join(web, "sdk", "src", "contract.gen.ts");

const REF = "#/components/schemas/";

/** `text` as a JSDoc comment at `indent`, or nothing. */
function doc(text, indent = "") {
  if (!text) return "";
  const lines = String(text).replaceAll("*/", "*\\/").split("\n");
  if (lines.length === 1) return `${indent}/** ${lines[0]} */\n`;
  return `${indent}/**\n${lines.map(l => `${indent} *${l ? " " + l : ""}`).join("\n")}\n${indent} */\n`;
}

const IDENT = /^[A-Za-z_$][A-Za-z0-9_$]*$/;
const key = k => (IDENT.test(k) ? k : JSON.stringify(k));

/** Wraps a union or function type for use as an array's element. */
const element = t => (/[|&]/.test(t) ? `(${t})[]` : `${t}[]`);

const union = ts => {
  const seen = [...new Set(ts)];
  return seen.includes("unknown") ? "unknown" : seen.join(" | ");
};

/** The TypeScript type of JSON Schema `s`, at `indent`. */
export function typeOf(s, indent = "") {
  if (s === true || s === undefined) return "unknown";
  if (s === false) return "never";
  if (s.$ref) {
    if (!s.$ref.startsWith(REF)) throw new Error(`a $ref outside the components: ${s.$ref}`);
    return s.$ref.slice(REF.length);
  }
  if ("const" in s) return JSON.stringify(s.const);
  if (s.enum) return union(s.enum.map(v => JSON.stringify(v)));
  const alts = s.anyOf ?? s.oneOf;
  if (alts) return union(alts.map(a => typeOf(a, indent)));
  if (Array.isArray(s.type)) return union(s.type.map(t => typeOf({ ...s, type: t }, indent)));
  switch (s.type) {
    case "string": return "string";
    case "integer":
    case "number": return "number";
    case "boolean": return "boolean";
    case "null": return "null";
    case "array": return element(typeOf(s.items, indent));
    case "object": return objectType(s, indent);
    case undefined: return s.properties ? objectType(s, indent) : "unknown";
    default: throw new Error(`a schema of type ${s.type}`);
  }
}

/** An object schema's members, one per line at `indent` + 2. */
function members(s, indent) {
  const inner = indent + "  ";
  const required = new Set(s.required ?? []);
  return Object.entries(s.properties ?? {})
    .map(([k, p]) => `${doc(p.description, inner)}${inner}${key(k)}${required.has(k) ? "" : "?"}: ${typeOf(p, inner)};\n`)
    .join("");
}

function objectType(s, indent) {
  if (!s.properties) {
    const extra = s.additionalProperties;
    return `Record<string, ${extra && extra !== true ? typeOf(extra, indent) : "unknown"}>`;
  }
  return `{\n${members(s, indent)}${indent}}`;
}

/** The contract's error codes of operation `o`, by status. */
function errorCodes(o) {
  return Object.entries(o.responses)
    .filter(([status]) => Number(status) >= 400)
    .flatMap(([, r]) => r["x-clax-codes"] ?? []);
}

/** The JSON body type of `content`, or `never` for none, `Blob` for bytes. */
function bodyType(content) {
  if (!content) return "never";
  if (content["application/json"]) return typeOf(content["application/json"].schema, "    ");
  if (content["multipart/form-data"]) return "FormData";
  return "Blob";
}

/** contract.gen.ts of the contract `c` (openapi.json, parsed). */
export function generate(c) {
  const out = [];
  out.push(`// Generated from web/sdk/openapi.json by web/scripts/contract-types.mjs: do not edit.
// The Clax API contract (spec 2026-10-27-clax-sdk-design §4.1), version ${c.info.version}.
// Regenerate with \`just contract\`.

/** The contract's version: its major is what \`GET /.well-known/clax\` serves as \`contract_version\`. */
export type ContractVersion = ${JSON.stringify(c.info.version)};
`);
  for (const [name, s] of Object.entries(c.components.schemas)) {
    const isObject = (s.type === "object" || s.properties) && s.properties && !s.anyOf && !s.oneOf;
    out.push("\n", doc(s.description));
    out.push(isObject ? `export interface ${name} ${objectType(s, "")}\n` : `export type ${name} = ${typeOf(s)};\n`);
  }
  out.push(`
/** Each public operation, by \`operationId\`: its method and path, its
 * bodies, the \`error.code\`s it answers, and the contract's \`x-clax-*\`
 * extensions (retry, channel, rights, stream). */
export interface Operations {
`);
  const ops = [];
  for (const [path, item] of Object.entries(c.paths)) {
    for (const [method, o] of Object.entries(item)) ops.push({ path, method, o });
  }
  ops.sort((a, b) => a.o.operationId.localeCompare(b.o.operationId));
  for (const { path, method, o } of ops) {
    const ok = Object.entries(o.responses).find(([st]) => Number(st) < 400);
    const query = (o.parameters ?? []).filter(p => p.in === "query");
    const codes = [...new Set(errorCodes(o))].sort();
    const rights = o["x-clax-rights"];
    out.push(`${doc(o.summary, "  ")}  ${key(o.operationId)}: {
    method: ${JSON.stringify(method.toUpperCase())};
    path: ${JSON.stringify(path)};
    query: ${query.length ? `{ ${query.map(p => `${key(p.name)}${p.required ? "" : "?"}: ${typeOf(p.schema)}`).join("; ")} }` : "never"};
    request: ${bodyType(o.requestBody?.content)};
    status: ${ok[0]};
    response: ${bodyType(ok[1].content)};
    errors: ${codes.length ? codes.map(code => JSON.stringify(code)).join(" | ") : "never"};
    retry: ${o["x-clax-retry"]};
    channel: ${o["x-clax-channel"].map(ch => JSON.stringify(ch)).join(" | ")};
    rights: ${rights === null ? "null" : `{ ${Object.entries(rights).map(([k, v]) => `${key(k)}: ${JSON.stringify(v)}`).join("; ")} }`};
    stream: ${o["x-clax-stream"]};
  };
`);
  }
  out.push(`}

/** Every \`error.code\` an operation of the contract answers. */
export type ErrorCode = Operations[keyof Operations]["errors"];
`);
  return out.join("");
}

if (process.argv[1] && import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href) {
  const text = generate(JSON.parse(readFileSync(CONTRACT, "utf8")));
  if (process.argv.includes("--check")) {
    if (readFileSync(GENERATED, "utf8") !== text) {
      console.error("web/sdk/src/contract.gen.ts is not what web/sdk/openapi.json generates: run `just contract`");
      process.exit(1);
    }
  } else {
    writeFileSync(GENERATED, text);
  }
}
