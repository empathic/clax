// A manual check that the API key store works on a macOS build: it writes,
// reads back and deletes a probe value with the same options as
// FlutterSecureKey (lib/platform/secure_key.dart), under its own storage
// key so a saved API key is never touched.
//
//   flutter build macos --debug -t tool/secure_key_probe.dart
//   build/macos/Build/Products/Debug/clax_app.app/Contents/MacOS/clax_app
//
// Prints one line per step and PASS or FAIL; exits 0 on PASS.
import 'dart:io';

import 'package:clax_app/platform/secure_key.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  const storage = FlutterSecureStorage(mOptions: FlutterSecureKey.macOsOptions);
  const key = 'clax_secure_key_probe';
  final value = 'probe-${DateTime.now().microsecondsSinceEpoch}';
  var ok = false;
  try {
    await storage.write(key: key, value: value);
    stdout.writeln('secure-key-probe: write ok');
    final back = await storage.read(key: key);
    stdout.writeln(
      'secure-key-probe: read ${back == value ? 'matches' : 'differs'}',
    );
    await storage.delete(key: key);
    final gone = await storage.read(key: key);
    stdout.writeln(
      'secure-key-probe: delete ${gone == null ? 'ok' : 'left a value'}',
    );
    ok = back == value && gone == null;
  } catch (e) {
    stdout.writeln('secure-key-probe: error: $e');
  }
  stdout.writeln(ok ? 'secure-key-probe: PASS' : 'secure-key-probe: FAIL');
  await stdout.flush();
  exit(ok ? 0 : 1);
}
