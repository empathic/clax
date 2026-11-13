import { afterEach, describe, expect, it } from "vitest";
import { type GalleryBoot, answered, readGalleryBoot } from "./gallery-boot";

const boot = (): GalleryBoot => ({
  v: 1,
  kind: "gallery",
  status: { viewer: 200, hosted: 404, feedback: 200, artifacts: 200, attention: 200 },
  viewer: { viewer: { public_id: "v1", display_name: "Ada </script>", person: true, person_revoked_at: null }, may_create: true },
  hosted: null,
  feedback: { enabled: false, instance: null, queue: null, context: null },
  inbox: true,
  artifacts: { artifacts: [] },
  attention: { artifacts: { a1: { addressed: [], addressed_v: null, new_replies: [], open_in: ["t1"], seen: 1 } } },
});

describe("readGalleryBoot", () => {
  afterEach(() => { document.head.replaceChildren(); });
  const block = (text: string) => {
    const s = document.createElement("script");
    s.type = "application/json";
    s.id = "clax-boot";
    s.textContent = text;
    document.head.append(s);
  };

  it("reads the daemon's block, escapes and all", () => {
    expect(readGalleryBoot(document, "navigate")).toBeNull();
    // As the daemon escapes it (script_json): no `<`, `>` or `&` in the element.
    block(JSON.stringify(boot()).replace(/</g, "\\u003c").replace(/>/g, "\\u003e").replace(/&/g, "\\u0026"));
    expect(readGalleryBoot(document, "navigate")).toEqual(boot());
    expect(readGalleryBoot(document, "reload")).toEqual(boot());
  });

  it("tells a route's refusal from a field the shell must ask for", () => {
    const { status, ...rest } = boot();
    block(JSON.stringify({ ...rest, hosted: null, status: { ...status, viewer: undefined } }));
    const b = readGalleryBoot(document, "navigate")!;
    expect(answered(b, "hosted")).toBe(true);
    expect(b.hosted).toBeNull();
    expect(answered(b, "viewer")).toBe(false);
  });

  it("leaves out what depends on the viewer of a page reached through history, which may come from the cache", () => {
    block(JSON.stringify(boot()));
    const b = readGalleryBoot(document, "back_forward")!;
    expect(b.viewer).toBeNull();
    expect(b.hosted).toBeNull();
    expect(b.feedback).toBeNull();
    expect(b.attention).toBeNull();
    expect(b.inbox).toBeNull();
    for (const f of ["viewer", "hosted", "feedback", "attention"] as const) expect(answered(b, f), f).toBe(false);
    expect(b.artifacts).toEqual(boot().artifacts);
    expect(answered(b, "artifacts")).toBe(true);
    // The block itself is left as it was.
    expect(readGalleryBoot(document, "navigate")).toEqual(boot());
  });

  it("keeps a page's absent list fields absent through history", () => {
    const header: Partial<GalleryBoot> = { ...boot(), status: { viewer: 200, hosted: 404, feedback: 200 } };
    delete header.artifacts;
    delete header.attention;
    block(JSON.stringify(header));
    const b = readGalleryBoot(document, "back_forward")!;
    expect("attention" in b).toBe(false);
    expect("artifacts" in b).toBe(false);
  });

  it("refuses a block that does not parse, is not version 1, or is the artifact page's", () => {
    block("{");
    expect(readGalleryBoot(document, "navigate")).toBeNull();
    for (const bad of [{ ...boot(), v: 2 }, { ...boot(), kind: undefined }, { ...boot(), status: undefined }, { v: 1, artifact: {}, threads: [] }]) {
      document.head.replaceChildren();
      block(JSON.stringify(bad));
      expect(readGalleryBoot(document, "navigate"), JSON.stringify(bad)).toBeNull();
    }
  });
});
