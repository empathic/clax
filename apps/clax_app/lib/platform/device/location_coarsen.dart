/// Approximate location for an app allowed only that: a fix snapped to the
/// centre of a cell of about 2 km, as Android's own approximate location
/// is, so a page never learns more than the area.
library;

import 'dart:math' as math;

import 'device_ports.dart';

/// Snaps fixes to a grid of cells [cellDegrees] of latitude tall (0.02°,
/// about 2.2 km) and as wide in kilometres (more degrees of longitude
/// toward the poles). The grid is shifted by [offsetLat] and [offsetLon]
/// (fractions of a cell, from 0 to 1), drawn at random for each app and
/// kept with its location grant, so a cell's edges are not at known lines
/// and stay put across runs of Clax.
///
/// A fix inside a cell always gives the cell's centre: averaging many
/// readings learns nothing finer.
class Coarsener {
  Coarsener({required this.offsetLat, required this.offsetLon});

  static const cellDegrees = 0.02;

  /// The accuracy an approximate fix reports at the least, in metres.
  static const minAccuracy = 2000.0;

  final double offsetLat;
  final double offsetLon;

  /// The cell of ([lat], [lon]): its row and column, and its centre.
  ({int row, int col, double lat, double lon}) cell(double lat, double lon) {
    final row = (lat / cellDegrees + offsetLat).floor();
    final centreLat = ((row + 0.5 - offsetLat) * cellDegrees).clamp(
      -90.0,
      90.0,
    );
    // Cells keep about the same width on the ground: wider in degrees
    // away from the equator, capped near the poles.
    final cos = math.cos(centreLat * math.pi / 180).abs().clamp(0.05, 1.0);
    final width = cellDegrees / cos;
    final col = (lon / width + offsetLon).floor();
    var centreLon = (col + 0.5 - offsetLon) * width;
    if (centreLon >= 180) centreLon -= 360;
    if (centreLon < -180) centreLon += 360;
    return (row: row, col: col, lat: centreLat, lon: centreLon);
  }

  /// [fix] as an app allowed approximate location sees it: its cell's
  /// centre, at least [minAccuracy], and not precise.
  LocationFix coarsen(LocationFix fix) {
    final c = cell(fix.lat, fix.lon);
    return LocationFix(
      _round(c.lat),
      _round(c.lon),
      math.max(minAccuracy, fix.accuracy).roundToDouble(),
      precise: false,
    );
  }

  /// Six decimals: the centre's floating-point noise is not a signal.
  static double _round(double v) => (v * 1e6).roundToDouble() / 1e6;
}
