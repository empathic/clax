import { INDEX_FILE } from "./protocol";

/** State the eager bridge shares with the `comments` capability's page-side
 * members: the frame's version and file, whether a custom-anchors
 * registration is live (the bridge's own comment mode then stands down), the
 * hook the bridge calls on scroll and resize, and the one it gives to hear
 * `live` change. Parts get this object as an argument and never import this
 * module: a part is a separate build with its own module instances. */
export type CommentsContext = {
  version: number;
  file: string;
  live: boolean;
  reflow: (() => void) | null;
  liveChanged: (() => void) | null;
};

export const commentsContext: CommentsContext = { version: 0, file: INDEX_FILE, live: false, reflow: null, liveChanged: null };
