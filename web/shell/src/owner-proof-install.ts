// Imported first by each shell entry, so every request the page makes
// carries the local owner browser session's proof (`owner-proof.ts`).
import { installProofFetch } from "./owner-proof";

installProofFetch();
