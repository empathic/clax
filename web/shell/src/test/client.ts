// @empathic/clax-client/testing: for a client's unit tests only: mounting a
// component under jsdom, the inbox's fixtures, and trusted events (which
// comment mode needs, and jsdom cannot make). With the tests' files
// (web/BUILD.bazel TEST_FILES), not the build's. Part of web/shell
// (AGPL-3.0-only).
export { mount, type Mounted } from "./svelte";
export * as fixtures from "../q/fixtures";
export { dispatchTrusted, trusted } from "../../../bridge/test/trusted";
