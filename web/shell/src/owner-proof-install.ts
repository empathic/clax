// Imported first by each shell entry, so every request the page makes
// carries the local owner browser session's proof (`owner-proof.ts`) and
// tells the connection notice what it found of Clax (`reach.ts`). On the
// Clax app's transport (`transport.ts`), whose core marks the shell's
// frames itself and which hides the notice, nothing is wrapped.
import { installProofFetch } from "./owner-proof";
import { heard, pending } from "./reach";
import { onHost } from "./transport";

if (!onHost()) installProofFetch(undefined, { heard, pending });
