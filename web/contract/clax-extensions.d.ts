/**
 * Clax's additions to runtime contract 0.2.61.
 *
 * The files under `0.2.61/` are claude.ai's type definitions, unchanged.
 * What Clax answers beyond them is declared here, so a page written for
 * claude.ai keeps working and a page written for Clax can handle the extra
 * cases. Served at `<daemon_url>/_clax/contract/clax-extensions.d.ts`.
 */
declare namespace ClaxExtensions {
  /**
   * An extra error code of the calls that act beyond a composer the viewer
   * sees: `artifact.publish` (`artifact.d.ts`), and `create`, `reply`,
   * `resolve`, `delete` and `sendToClaude` (`comments.d.ts`).
   *
   * - `shell_input_recent` — the call came from the viewer's click or key
   *   in the page, but within 5.5 s of their input to the Clax window
   *   around it (its name field, composer or buttons), so nothing was
   *   published or written. Keep any draft and tell the viewer to click
   *   again. It is not charged to any budget. A page that treats unknown
   *   codes as the contract says (`upstream_error`) still behaves
   *   correctly.
   */
  type ShellInputRecent = "shell_input_recent";

  /** The codes Clax adds to `ArtifactErrorCode` (`artifact.d.ts`). */
  type ArtifactErrorCode = ShellInputRecent;

  /**
   * The codes Clax adds to `CommentsErrorCode` (`comments.d.ts`).
   *
   * Clax also answers `unavailable` from `create`, `reply`, `resolve` and
   * `delete` called without the viewer's own click or key in the page
   * (`sendToClaude` answers `claude_unavailable`, as the contract says).
   * One retry from a fresh gesture is reasonable.
   */
  type CommentsErrorCode = ShellInputRecent;
}
