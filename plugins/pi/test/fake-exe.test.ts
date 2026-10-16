import { mkdirSync, mkdtempSync, readlinkSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, expect, test } from "vitest";
import { fakeExe } from "./fake-exe";

const saved = process.env.CLAX_FAKE_EXE_DIR;
const made: string[] = [];
afterEach(() => {
  if (saved === undefined) delete process.env.CLAX_FAKE_EXE_DIR;
  else process.env.CLAX_FAKE_EXE_DIR = saved;
  for (const d of made.splice(0)) rmSync(d, { recursive: true, force: true });
});

test("the store is CLAX_FAKE_EXE_DIR when set", () => {
  const d = mkdtempSync(join(tmpdir(), "clax-fake-test-"));
  made.push(d);
  process.env.CLAX_FAKE_EXE_DIR = join(d, "store");
  mkdirSync(join(d, "bin"), { recursive: true });
  const at = fakeExe(join(d, "bin", "hi"), "#!/bin/sh\necho hi\n");
  expect(readlinkSync(at).startsWith(join(d, "store") + "/")).toBe(true);
});
