/// Which device capabilities the person allowed each app, kept until they
/// revoke them (the app inspector lists them with [DeviceGrants.list] and
/// revokes with [DeviceGrants.revoke]).
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

/// The device capabilities, in the order they are listed.
const deviceCaps = ['notify', 'share', 'files', 'camera', 'location'];

/// What each device capability lets an app do, in words for the person.
const deviceCapWords = {
  'notify': 'send you notifications and reminders',
  'share': 'share text and files through other apps',
  'files': 'open files you pick and save files where you choose',
  'camera': 'take photos and pick photos from your library',
  'location': 'see where your device is',
};

abstract interface class DeviceGrants {
  /// Whether the person allowed app [aid] to use [cap].
  Future<bool> granted(String aid, String cap);

  Future<void> grant(String aid, String cap);

  /// Takes [cap] back from app [aid]; its next call asks again.
  Future<void> revoke(String aid, String cap);

  /// App [aid]'s grants, capability to when it was allowed, in
  /// [deviceCaps] order.
  Future<Map<String, DateTime>> list(String aid);

  /// The ID of each app whose grants changed.
  Stream<String> get changes;
}

/// [DeviceGrants] kept in memory, for tests and as a fallback.
class MemoryDeviceGrants implements DeviceGrants {
  MemoryDeviceGrants({DateTime Function()? now}) : _now = now ?? DateTime.now;

  final DateTime Function() _now;
  final _grants = <String, Map<String, DateTime>>{};
  final _changes = StreamController<String>.broadcast();

  @override
  Future<bool> granted(String aid, String cap) async =>
      _grants[aid]?.containsKey(cap) ?? false;

  @override
  Future<void> grant(String aid, String cap) async {
    (_grants[aid] ??= {})[cap] = _now();
    _changes.add(aid);
  }

  @override
  Future<void> revoke(String aid, String cap) async {
    if (_grants[aid]?.remove(cap) != null) _changes.add(aid);
  }

  @override
  Future<Map<String, DateTime>> list(String aid) async =>
      _ordered(_grants[aid]);

  @override
  Stream<String> get changes => _changes.stream;
}

Map<String, DateTime> _ordered(Map<String, DateTime>? m) => {
  for (final c in deviceCaps) c: ?m?[c],
};

/// [DeviceGrants] in one JSON file (`<support>/device-grants.json`):
/// `{"<app ID>": {"<capability>": "<ISO time allowed>"}}`. A save writes a
/// sibling temporary file and renames it over the old one.
class FileDeviceGrants implements DeviceGrants {
  FileDeviceGrants(this.file, {DateTime Function()? now})
    : _now = now ?? DateTime.now;

  final File file;
  final DateTime Function() _now;
  final _changes = StreamController<String>.broadcast();

  Map<String, Map<String, DateTime>> _read() {
    try {
      final j = jsonDecode(file.readAsStringSync());
      if (j is! Map) return {};
      return {
        for (final MapEntry(:key, :value) in j.entries)
          if (key is String && value is Map)
            key: {
              for (final MapEntry(key: c, value: at) in value.entries)
                if (c is String && deviceCaps.contains(c) && at is String)
                  c: ?DateTime.tryParse(at),
            },
      };
    } on FileSystemException {
      return {};
    } on FormatException {
      return {};
    }
  }

  void _write(Map<String, Map<String, DateTime>> all) {
    file.parent.createSync(recursive: true);
    final tmp = File('${file.path}.tmp');
    tmp.writeAsStringSync(
      jsonEncode({
        for (final MapEntry(:key, :value) in all.entries)
          if (value.isNotEmpty)
            key: {
              for (final MapEntry(key: c, value: at) in value.entries)
                c: at.toUtc().toIso8601String(),
            },
      }),
      flush: true,
    );
    tmp.renameSync(file.path);
  }

  @override
  Future<bool> granted(String aid, String cap) async =>
      _read()[aid]?.containsKey(cap) ?? false;

  @override
  Future<void> grant(String aid, String cap) async {
    final all = _read();
    (all[aid] ??= {})[cap] = _now();
    _write(all);
    _changes.add(aid);
  }

  @override
  Future<void> revoke(String aid, String cap) async {
    final all = _read();
    if (all[aid]?.remove(cap) == null) return;
    _write(all);
    _changes.add(aid);
  }

  @override
  Future<Map<String, DateTime>> list(String aid) async =>
      _ordered(_read()[aid]);

  @override
  Stream<String> get changes => _changes.stream;
}
