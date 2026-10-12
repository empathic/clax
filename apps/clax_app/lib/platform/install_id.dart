import 'dart:io';
import 'dart:math';

/// The app's install ID: a random UUID (version 4) kept in `<support>/install-id`.
///
/// The daemon finds the app's session again by it after either restarts,
/// so it is made once per install and kept. A file that is missing or does
/// not hold a UUID is replaced by a new one.
Future<String> loadInstallId(Directory support, {Random? random}) async {
  final file = File('${support.path}/install-id');
  try {
    final kept = file.readAsStringSync().trim();
    if (_uuid.hasMatch(kept)) return kept;
  } on FileSystemException {
    // Not made yet.
  }
  final id = newUuid(random ?? Random.secure());
  support.createSync(recursive: true);
  final tmp = File('${file.path}.tmp');
  tmp.writeAsStringSync(id, flush: true);
  tmp.renameSync(file.path);
  return id;
}

final _uuid = RegExp(
  r'^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$',
);

/// A random UUID, version 4, in lowercase.
String newUuid(Random random) {
  final b = [for (var i = 0; i < 16; i++) random.nextInt(256)];
  b[6] = (b[6] & 0x0f) | 0x40;
  b[8] = (b[8] & 0x3f) | 0x80;
  final h = [for (final x in b) x.toRadixString(16).padLeft(2, '0')].join();
  return '${h.substring(0, 8)}-${h.substring(8, 12)}-${h.substring(12, 16)}-'
      '${h.substring(16, 20)}-${h.substring(20)}';
}
