// Signing out of a hosted Clax: the person's reporter key and token are
// forgotten first (`feedback/forget.ts`, a chunk loaded only here), and the
// gallery's remembered page shapes (`view/gallery-shape.ts`: the counts of
// what this person saw), and the sign-out goes on whatever that does: a
// chunk that fails to load, or a forgetting that fails or runs out of
// time, never stops it.
import { forgetShapes } from "../view/gallery-shape";

/** Forgets the person's reporter and the gallery's shapes, then runs `signOut`. */
export async function forgetThenSignOut<T>(
  signOut: () => Promise<T>,
  load: () => Promise<{ forgetFeedback(): Promise<void> }> = () => import("../feedback/forget"),
): Promise<T> {
  forgetShapes();
  await load().then(m => m.forgetFeedback()).catch(() => {});
  return signOut();
}
