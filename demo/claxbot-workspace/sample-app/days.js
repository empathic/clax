// __PRIVATE_MARKER__: the leave rules of this company, private to it.
// How many days a leave request takes, both ends included: a leave from a
// Monday to the same Monday is 1 day.
export function daysBetween(start, end) {
  const ms = new Date(end) - new Date(start);
  return Math.round(ms / 86400000);
}
