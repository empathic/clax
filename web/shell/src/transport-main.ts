// `/_clax/transport.js` (`vite.transport.config.ts`): the app's transport,
// which the core writes as a classic script into every shell document it
// serves in the app, before the shell's modules (`transport.ts`).
import { installTransport } from "./transport";

installTransport();
