// A manual look at the launcher, the New sheet, the making screen and
// Settings, over an in-memory daemon (FakeClaxApi) with a few apps. Making
// an app waits three seconds and fails on a request containing "fail", so
// both outcomes can be seen. Settings are kept in a temporary directory;
// the key field is backed by an in-memory store, so nothing reaches the
// keychain.
//
//   flutter run -d macos -t tool/launcher_preview.dart
import 'dart:io';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/platform/secure_key.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/platform/voice.dart';
import 'package:clax_app/ui/launcher.dart';
import 'package:clax_app/ui/settings_screen.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/material.dart';

class _MemoryKey implements SecureKey {
  String? _v;
  @override
  Future<String?> read() async => _v;
  @override
  Future<void> write(String? key) async => _v = key;
}

void main() {
  final api = FakeClaxApi(
    clock: () => DateTime.now().toUtc().toIso8601String(),
  );
  const page = {'index.html': '<!doctype html><title>x</title>'};
  for (final t in ['Run log', 'Groceries', 'Reading list', 'Water']) {
    api.seedArtifact(t, page);
  }
  final store = SettingsStore(
    File('${Directory.systemTemp.createTempSync('clax-preview-').path}/s.json'),
  );
  final key = _MemoryKey();
  final nav = GlobalKey<NavigatorState>();
  runApp(
    MaterialApp(
      navigatorKey: nav,
      title: 'Clax',
      theme: claxTheme(Brightness.light),
      darkTheme: claxTheme(Brightness.dark),
      home: Launcher(
        api: api,
        voice: SpeechToTextVoice(),
        open: (aid) =>
            ScaffoldMessenger.of(nav.currentContext!)
                .showSnackBar(SnackBar(content: Text('Would open $aid'))),
        make: (request) async {
          await Future<void>.delayed(const Duration(seconds: 3));
          if (request.contains('fail')) {
            throw const AgentError('Add your Claude API key in Settings');
          }
          return api.seedArtifact(request, page);
        },
        openSettings: () => nav.currentState!.push(
          MaterialPageRoute(
            builder: (_) => SettingsScreen(
              store: store,
              secureKey: key,
              cliSupported: Platform.isMacOS,
              timings: [
                TimingRecord(
                  name: sayUsable,
                  elapsed: const Duration(milliseconds: 8400),
                  at: DateTime.now(),
                ),
              ],
            ),
          ),
        ),
      ),
    ),
  );
}
