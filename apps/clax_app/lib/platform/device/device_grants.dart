/// What the person answered each app for each device capability, Allow or
/// Don't allow (for location: Precise, Approximate or Don't allow), kept
/// until they revoke it (the app inspector lists the
/// answers with [DeviceGrants.list] and revokes with
/// [DeviceGrants.revoke]; then the next call asks again).
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'device_ports.dart' show Precision;

export 'device_ports.dart' show Precision;

/// The device capabilities, in the order they are listed.
const deviceCaps = ['notify', 'share', 'file', 'camera', 'location'];

/// What each device capability lets an app do, in words for the person.
const deviceCapWords = {
  'notify': 'send you notifications and reminders',
  'share': 'share text and files through other apps',
  'file': 'open files you pick and save files where you choose',
  'camera': 'take photos and pick photos from your library',
  'location': 'see where your device is',
};

/// The person's answer for one app and capability, and when; for an
/// allowed `location`, the [precision] it was allowed at (null otherwise)
/// and the app's approximate-grid [shift] (fractions of a cell, from 0 to
/// 1; `location_coarsen.dart`), kept until the answer is revoked or the
/// app deleted, so the app's cells stay the same across runs of Clax.
class DeviceDecision {
  const DeviceDecision({
    required this.allowed,
    required this.at,
    this.precision,
    this.shift,
  });

  final bool allowed;
  final DateTime at;
  final Precision? precision;
  final (double, double)? shift;

  @override
  bool operator ==(Object other) =>
      other is DeviceDecision &&
      other.allowed == allowed &&
      other.at == at &&
      other.precision == precision &&
      other.shift == shift;

  @override
  int get hashCode => Object.hash(allowed, at, precision, shift);

  @override
  String toString() =>
      'DeviceDecision(allowed: $allowed, at: $at, precision: $precision, '
      'shift: $shift)';
}

/// [precision] as kept for [cap]: an allowed location's, Precise when not
/// given; null for anything else.
Precision? _precisionFor(String cap, bool allowed, Precision? precision) =>
    cap == 'location' && allowed ? (precision ?? Precision.precise) : null;

DeviceDecision _withShift(DeviceDecision d, (double, double) shift) =>
    DeviceDecision(
      allowed: d.allowed,
      at: d.at,
      precision: d.precision,
      shift: shift,
    );

(double, double)? _parseShift(Object? v) =>
    v is List && v.length == 2 && v.every((x) => x is num && x >= 0 && x < 1)
    ? ((v[0] as num).toDouble(), (v[1] as num).toDouble())
    : null;

abstract interface class DeviceGrants {
  /// The person's answer for app [aid] and [cap]; null when never asked
  /// (or revoked since).
  Future<DeviceDecision?> decision(String aid, String cap);

  /// Keeps the person's answer for app [aid] and [cap]; an allowed
  /// `location` keeps [precision] (Precise when null) and [shift].
  Future<void> decide(
    String aid,
    String cap, {
    required bool allowed,
    Precision? precision,
    (double, double)? shift,
  });

  /// Gives app [aid]'s allowed location answer [shift] (one kept before
  /// shifts existed), leaving the rest of it as it is.
  Future<void> keepShift(String aid, (double, double) shift);

  /// Forgets app [aid]'s answer for [cap]; its next call asks again.
  Future<void> revoke(String aid, String cap);

  /// Forgets every answer of app [aid] (it was deleted).
  Future<void> forget(String aid);

  /// App [aid]'s answers, in [deviceCaps] order.
  Future<Map<String, DeviceDecision>> list(String aid);

  /// The ID of each app whose answers changed.
  Stream<String> get changes;
}

Map<String, DeviceDecision> _ordered(Map<String, DeviceDecision>? m) => {
  for (final c in deviceCaps) c: ?m?[c],
};

/// [DeviceGrants] kept in memory, for tests and as a fallback.
class MemoryDeviceGrants implements DeviceGrants {
  MemoryDeviceGrants({DateTime Function()? now}) : _now = now ?? DateTime.now;

  final DateTime Function() _now;
  final _all = <String, Map<String, DeviceDecision>>{};
  final _changes = StreamController<String>.broadcast();

  @override
  Future<DeviceDecision?> decision(String aid, String cap) async =>
      _all[aid]?[cap];

  @override
  Future<void> decide(
    String aid,
    String cap, {
    required bool allowed,
    Precision? precision,
    (double, double)? shift,
  }) async {
    (_all[aid] ??= {})[cap] = DeviceDecision(
      allowed: allowed,
      at: _now(),
      precision: _precisionFor(cap, allowed, precision),
      shift: _precisionFor(cap, allowed, precision) == null ? null : shift,
    );
    _changes.add(aid);
  }

  @override
  Future<void> keepShift(String aid, (double, double) shift) async {
    final d = _all[aid]?['location'];
    if (d == null || !d.allowed) return;
    _all[aid]!['location'] = _withShift(d, shift);
  }

  @override
  Future<void> revoke(String aid, String cap) async {
    if (_all[aid]?.remove(cap) != null) _changes.add(aid);
  }

  @override
  Future<void> forget(String aid) async {
    if (_all.remove(aid) != null) _changes.add(aid);
  }

  @override
  Future<Map<String, DeviceDecision>> list(String aid) async =>
      _ordered(_all[aid]);

  @override
  Stream<String> get changes => _changes.stream;
}

/// [DeviceGrants] in one JSON file (`<support>/device-grants.json`):
/// `{"<app ID>": {"<capability>": {"allowed": bool, "at": "<ISO time>"}}}`,
/// with `"precision": "precise" | "approximate"` on an allowed `location`.
/// An allowed `location` kept before precisions existed (no `precision`)
/// reads as Precise, and is written with it on the next save.
/// A save writes a sibling temporary file and renames it over the old one.
class FileDeviceGrants implements DeviceGrants {
  FileDeviceGrants(this.file, {DateTime Function()? now})
    : _now = now ?? DateTime.now;

  final File file;
  final DateTime Function() _now;
  final _changes = StreamController<String>.broadcast();

  static DeviceDecision? _parse(String cap, Object? v) {
    if (v is! Map || v['allowed'] is! bool || v['at'] is! String) return null;
    final at = DateTime.tryParse(v['at'] as String);
    final allowed = v['allowed'] as bool;
    return at == null
        ? null
        : DeviceDecision(
            allowed: allowed,
            at: at,
            precision: _precisionFor(
              cap,
              allowed,
              v['precision'] == 'approximate' ? Precision.approximate : null,
            ),
            shift: cap == 'location' && allowed
                ? _parseShift(v['shift'])
                : null,
          );
  }

  Map<String, Map<String, DeviceDecision>> _read() {
    try {
      final j = jsonDecode(file.readAsStringSync());
      if (j is! Map) return {};
      return {
        for (final MapEntry(:key, :value) in j.entries)
          if (key is String && value is Map)
            key: {
              for (final MapEntry(key: c, value: d) in value.entries)
                if (c is String && deviceCaps.contains(c)) c: ?_parse(c, d),
            },
      };
    } on FileSystemException {
      return {};
    } on FormatException {
      return {};
    }
  }

  void _write(Map<String, Map<String, DeviceDecision>> all) {
    file.parent.createSync(recursive: true);
    final tmp = File('${file.path}.tmp');
    tmp.writeAsStringSync(
      jsonEncode({
        for (final MapEntry(:key, :value) in all.entries)
          if (value.isNotEmpty)
            key: {
              for (final MapEntry(key: c, value: d) in value.entries)
                c: {
                  'allowed': d.allowed,
                  'at': d.at.toUtc().toIso8601String(),
                  'precision': ?d.precision?.name,
                  if (d.shift case (final a, final b)) 'shift': [a, b],
                },
            },
      }),
      flush: true,
    );
    tmp.renameSync(file.path);
  }

  @override
  Future<DeviceDecision?> decision(String aid, String cap) async =>
      _read()[aid]?[cap];

  @override
  Future<void> decide(
    String aid,
    String cap, {
    required bool allowed,
    Precision? precision,
    (double, double)? shift,
  }) async {
    final all = _read();
    (all[aid] ??= {})[cap] = DeviceDecision(
      allowed: allowed,
      at: _now(),
      precision: _precisionFor(cap, allowed, precision),
      shift: _precisionFor(cap, allowed, precision) == null ? null : shift,
    );
    _write(all);
    _changes.add(aid);
  }

  @override
  Future<void> keepShift(String aid, (double, double) shift) async {
    final all = _read();
    final d = all[aid]?['location'];
    if (d == null || !d.allowed) return;
    all[aid]!['location'] = _withShift(d, shift);
    _write(all);
  }

  @override
  Future<void> revoke(String aid, String cap) async {
    final all = _read();
    if (all[aid]?.remove(cap) == null) return;
    _write(all);
    _changes.add(aid);
  }

  @override
  Future<void> forget(String aid) async {
    final all = _read();
    if (all.remove(aid) == null) return;
    _write(all);
    _changes.add(aid);
  }

  @override
  Future<Map<String, DeviceDecision>> list(String aid) async =>
      _ordered(_read()[aid]);

  @override
  Stream<String> get changes => _changes.stream;
}
