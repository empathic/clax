/** The top bar's running count of this artifact's calls to Claude today. */
export function callCountText(n: number, cap: number | null): string {
  return cap === null ? `${n} Claude call${n === 1 ? "" : "s"} today` : `${n} of ${cap} Claude calls today`;
}
