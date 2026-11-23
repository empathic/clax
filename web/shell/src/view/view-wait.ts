// How long a page beside the gallery keeps the view it shows while the
// next one gets its first data (a Settings screen moved to, a queue's or a
// report's view, the queues' counts): past it the next view shows as it
// stands, saying what it is still reading, so a slow or hung answer never
// leaves the old view up, or the page's skeleton, for good.

/** The longest a view moved to stays unseen (on the shell's clock, `../clock` `after`). */
export const VIEW_WAIT_MS = 300;
