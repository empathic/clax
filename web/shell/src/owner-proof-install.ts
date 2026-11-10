// Imported first by each shell entry, so every request the page makes
// carries the local owner browser session's proof (`owner-proof.ts`). On
// the Clax app's transport (`transport.ts`), whose core marks the shell's
// frames itself, nothing is wrapped.
import { installProofFetch } from "./owner-proof";
import { onHost } from "./transport";

if (!onHost()) installProofFetch();
