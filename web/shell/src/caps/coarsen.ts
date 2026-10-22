// Approximate location in the shell, for a page the person allowed only
// that, where the shell reads the position itself (WebKit's own
// `navigator.geolocation`, on iOS): the rule of the app's `Coarsener`
// (apps/clax_app/lib/platform/device/location_coarsen.dart), cell for cell.
// A fix snaps to the centre of a cell 0.02° of latitude tall (about
// 2.2 km) and as wide on the ground, on a grid shifted by a fraction of a
// cell that the app draws once per install and sends with the grant, so
// the cells' edges are the app's own on every platform.

/** A cell's height in degrees of latitude. */
export const CELL_DEGREES = 0.02;
/** How far inside its cell (a fraction of it) a point must be to count as
 * having moved there. */
export const MARGIN = 0.2;
/** The least accuracy an approximate fix reports, in metres. */
export const MIN_ACCURACY = 2000;

export type Cell = { row: number; col: number; lat: number; lon: number; deep: boolean };
export type Fix = { lat: number; lon: number; accuracy: number; precise: boolean };

/** Dart's `roundToDouble`: halves away from zero. */
const round = (v: number) => Math.sign(v) * Math.round(Math.abs(v));

export class Coarsener {
  /** `offsetLat` and `offsetLon` are the grid's shift, fractions of a cell
   * from 0 to 1. */
  constructor(readonly offsetLat: number, readonly offsetLon: number) {}

  /** The cell of (`lat`, `lon`): its row and column, its centre, and
   * whether the point lies `deep` inside it (`MARGIN` from every edge). */
  cell(lat: number, lon: number): Cell {
    const rowAt = lat / CELL_DEGREES + this.offsetLat;
    const row = Math.floor(rowAt);
    const centreLat = Math.min(90, Math.max(-90, (row + 0.5 - this.offsetLat) * CELL_DEGREES));
    const cos = Math.min(1, Math.max(0.05, Math.abs(Math.cos(centreLat * Math.PI / 180))));
    const width = CELL_DEGREES / cos;
    const colAt = lon / width + this.offsetLon;
    const col = Math.floor(colAt);
    const inside = (f: number) => f >= MARGIN && f <= 1 - MARGIN;
    const deep = inside(rowAt - row) && inside(colAt - col);
    let centreLon = (col + 0.5 - this.offsetLon) * width;
    if (centreLon >= 180) centreLon -= 360;
    if (centreLon < -180) centreLon += 360;
    return { row, col, lat: centreLat, lon: centreLon, deep };
  }

  /** `fix` as an app allowed approximate location sees it: its cell's
   * centre to six decimals, at least `MIN_ACCURACY`, not precise. */
  coarsen(fix: { lat: number; lon: number; accuracy: number }): Fix {
    const c = this.cell(fix.lat, fix.lon);
    return { lat: round(c.lat * 1e6) / 1e6, lon: round(c.lon * 1e6) / 1e6, accuracy: round(Math.max(MIN_ACCURACY, fix.accuracy)), precise: false };
  }
}
