import { isAbsolute, join } from "node:path";
import { defineConfig } from "@playwright/test";

// Under Bazel the paths it hands over are relative to the runfiles root
// (as e2e/bazel-env.ts does for the main suite).
const runfiles = process.env.JS_BINARY__RUNFILES;
if (runfiles) {
  const root = join(runfiles, process.env.JS_BINARY__WORKSPACE ?? "_main");
  for (const name of ["CLAX_GALLERY_DIST", "PLAYWRIGHT_BROWSERS_PATH"]) {
    const value = process.env[name];
    if (value && !isAbsolute(value)) process.env[name] = join(root, value);
  }
}

// The design gallery's visual-regression gate (design-gallery/gallery.spec.ts,
// //web:e2e_design_gallery): each section of the gallery's static build, in
// light and dark, at a desktop and a phone width, against the baselines
// committed in design-gallery/__screenshots__/<platform>/. With
// CLAX_GALLERY_UPDATE=1 the run writes new baselines into the test's
// outputs instead (Bazel zips them into test.outputs/outputs.zip), for a
// person to look at and commit. Under Bazel the HTML report, with the
// expected, actual and diff images of each failure, is kept in the
// outputs too, to be published beside the gallery.
const outputs = process.env.TEST_UNDECLARED_OUTPUTS_DIR;
const update = process.env.CLAX_GALLERY_UPDATE === "1";

export default defineConfig({
  testDir: "design-gallery",
  testMatch: /gallery\.spec\.ts$/,
  // Two at a time: each takes eleven screenshots, the contrast table's tall.
  fullyParallel: true,
  workers: 2,
  timeout: 120_000,
  expect: { timeout: 20_000, toHaveScreenshot: { maxDiffPixelRatio: 0, animations: "disabled", caret: "hide" } },
  snapshotPathTemplate: update && outputs ? join(outputs, "baselines", "{platform}", "{arg}{ext}") : "{testDir}/__screenshots__/{platform}/{arg}{ext}",
  ...(update ? { updateSnapshots: "all" as const } : {}),
  use: { browserName: "chromium" },
  ...(outputs ? { outputDir: join(outputs, "test-results") } : {}),
  reporter: outputs ? [["list"], ["html", { outputFolder: join(outputs, "report"), open: "never" }]] : "list",
});
