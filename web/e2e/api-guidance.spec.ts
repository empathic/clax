// The tests' `api()` helper (fixtures.ts) and the daemon's guidance answer:
// an agent's reply on a thread not sent to it is answered 200 `{guidance}`
// and writes nothing, so `api()` fails on it unless the caller expects one.
import { type Daemon, api, expect, publishAs, registerSession, test } from "./fixtures";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

test("api() fails on an answer that is only guidance, unless the caller expects one", async ({ page }) => {
  const s = await registerSession(d.base, d.token, "claude", "guidance-e2e");
  const { artifact } = await publishAs(d.base, d.token, s.id, "Guidance", { "index.html": "<main><h2>Goals</h2></main>" });
  await page.goto(`${d.base}/a/${artifact.id}`);
  // A thread the browser makes and does not send to the agent.
  const tid = await page.evaluate(async a => {
    const form = new FormData();
    form.set("anchor", JSON.stringify({ kind: "element", selector: "body > main > h2", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
    form.set("body", "Not sent");
    form.set("version", "1");
    const res = await fetch(`/api/artifacts/${a}/threads`, { method: "POST", body: form });
    if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
    return (await res.json()).thread.id as string;
  }, artifact.id);
  const path = `/api/artifacts/${artifact.id}/threads/${tid}/comments`;
  const reply = JSON.stringify({ body: "Done.", author_kind: "agent" });

  await expect(api(d.base, d.token, path, { method: "POST", session: s.id, body: reply })).rejects.toThrow(/answered guidance/);
  const answer = await api(d.base, d.token, path, { method: "POST", session: s.id, body: reply, guidance: true });
  expect(typeof answer.guidance).toBe("string");
  // Neither wrote a comment.
  const { thread } = await api(d.base, d.token, `/api/artifacts/${artifact.id}/threads/${tid}`);
  expect(thread.comments.map((c: { body: string }) => c.body)).toEqual(["Not sent"]);
});
