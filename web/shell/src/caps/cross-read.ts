// `db.artifact(id)`'s consent in the shell: app A, the artifact this view
// serves, reads app B's documents through A's own site (the bridge's
// `read`, web/bridge/src/caps/origin.ts); when the site refuses the read
// `not_granted`, the bridge asks the shell (`db.grant`) and reads again once
// it is granted. A lazy chunk, loaded on the first ask.
//
// The shell is the consent boundary, so:
// - B must be listed in A's `capabilities.db.read`, from the view's own
//   declaration (env.declared), never from anything the page sends;
// - only the owner shell (it holds the token) asks, once per source and
//   view ("Let “A” read “B”'s data?"); Allow makes the grant
//   (`PUT /api/artifacts/<A>/db-grants/<B>`), Don't allow stands for the
//   view. A dismissed prompt is not kept: the next read asks again. A viewer who is not the owner is never
//   asked, nor is anyone in the app's inspect view (`?app=1&inspect=1`),
//   which renders a draft for the agent: there a refusal stands.
import { inspectQuery } from "../route";
import { CapError } from "./errors";
import { ALWAYS } from "./grants";
import type { CapEnv } from "./host";

const ARTIFACT_ID = /^[0-9a-hjkmnp-tv-z]{12}$/;

/** Per view (its env): each source's answer, asked at most once, except that an Allow seen again (a read refused after it) means the grant was revoked, so it asks again. */
const asked = new WeakMap<CapEnv, Map<string, Promise<boolean>>>();
/** The sources whose Allow was saved, per view. */
const allowed = new WeakMap<CapEnv, Set<string>>();

/** Whether app `src` may now be read for the view's app: the owner allowed
 * it here (and the grant was saved). */
export async function grantRead(env: CapEnv, src: unknown): Promise<boolean> {
  if (typeof src !== "string" || !ARTIFACT_ID.test(src)) throw new CapError("invalid_argument", `'${String(src)}' is not an artifact ID`);
  const reads = env.declared.db?.read;
  if (src === env.aid || !Array.isArray(reads) || !reads.includes(src)) throw new CapError("not_declared", `this app does not declare reading ${src}'s documents (capabilities.db.read)`);
  if (env.token === null || inspectQuery(location.search)) return false;
  let mine = asked.get(env);
  if (!mine) asked.set(env, (mine = new Map()));
  let saved = allowed.get(env);
  if (!saved) allowed.set(env, (saved = new Set()));
  if (saved.delete(src)) mine.delete(src);
  let p = mine.get(src);
  if (!p) {
    p = ask(env, src).then(
      a => { if (a === "dismiss") mine!.delete(src); if (a === "allow") saved!.add(src); return a === "allow"; },
      e => { mine!.delete(src); throw e; },
    );
    mine.set(src, p);
  }
  return p;
}

/** The owner's answer; on Allow, once the grant is saved. */
async function ask(env: CapEnv, src: string): Promise<"allow" | "deny" | "dismiss"> {
  const headers: Record<string, string> = env.token ? { authorization: `Bearer ${env.token}` } : {};
  const info = (id: string) => fetch(`/api/artifacts/${id}`, { headers })
    .then(r => r.json()).then(j => (j as { artifact: { title: string; capabilities?: Record<string, unknown> } }).artifact, () => ({ title: "", capabilities: undefined }));
  const [a0, b0] = await Promise.all([info(env.aid), info(src)]);
  // A source without db has nothing a grant could open: no dialog.
  if (b0.capabilities && !Object.hasOwn(b0.capabilities, "db")) throw new CapError("not_declared", `${src} does not keep documents to share (it does not declare db)`);
  const [a, b] = [a0.title || env.aid, b0.title || src];
  const answer = await env.prompt({
    kind: "crossRead",
    title: `Let “${a}” read “${b}”'s data?`,
    body: `“${a}” will see the documents “${b}” keeps, and cannot change them. Documents private to one viewer stay private.`,
    allow: "Allow",
    deny: "Don't allow",
    always: ALWAYS,
  });
  if (answer !== "allow") return answer === "dismiss" ? "dismiss" : "deny";
  const res = await fetch(`/api/artifacts/${env.aid}/db-grants/${src}`, { method: "PUT", headers }).catch(() => null);
  if (!res?.ok) throw new CapError("unavailable", "the grant could not be saved");
  return "allow";
}
