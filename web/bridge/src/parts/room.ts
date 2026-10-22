// The `room` capability's page side and its socket on the page's own origin:
// loaded on the first claude.use("room") the site's declaration grants,
// never with the other capabilities' members (parts/caps.ts).
export { ROOM_METHODS, roomNamespace } from "../caps/room";
export { originRoom } from "../caps/room-socket";
