import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { svelteTesting } from "@testing-library/svelte/vite";
import { MOUNT_TIMEOUT_MS } from "./shell/src/test/timeouts";
export default defineConfig({
  // svelteTesting() resolves Svelte's browser build under jsdom and unmounts after each test.
  plugins: [svelte(), svelteTesting()],
  // The unit tests run the shell's clock with its test hook (shell/src/clock.ts);
  // the extension's worker test hook (__CLAX_EXT_TEST__) is off, as in its release build.
  define: { __CLAX_TEST_CLOCK__: "true", __CLAX_EXT_TEST__: "false" },
  // The bridge loads its lazy parts from source here (parts-static.ts).
  resolve: { alias: { "clax-bridge-parts": fileURLToPath(new URL("./bridge/src/parts-static.ts", import.meta.url)) } },
  // The home app's unit tests (apps/clax_app/home_unit) import its modules and the kit from outside web/.
  server: { fs: { allow: [fileURLToPath(new URL("..", import.meta.url))] } },
  // Every test and hook may take up to MOUNT_TIMEOUT_MS: a limit that ends a
  // test that hangs, well above what a loaded machine takes to load a fresh
  // module registry, so it never decides whether a test passes.
  test: { environment: "jsdom", testTimeout: MOUNT_TIMEOUT_MS, hookTimeout: MOUNT_TIMEOUT_MS, setupFiles: ["shell/test-setup.ts"], include: ["bridge/test/**/*.test.ts", "extension/src/**/*.test.ts", "shell/src/**/*.test.ts", "scripts/**/*.test.ts", "reel/**/*.test.ts", "../apps/clax_app/home_unit/**/*.test.js"] },
});
