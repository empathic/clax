// The views the Developer tools switch shows and hides, rendered with it
// off and on in every form and theme into CLAX_SHOTS_DIR (skipped without
// it): Settings scrolled to its Developer section, and the inspector's
// Overview and Permissions, and its Files tab, which shows only when on.
import 'dart:io';

import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/platform/secure_key.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_inspector.dart';
import 'package:clax_app/ui/developer.dart';
import 'package:clax_app/ui/settings_screen.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'rig.dart';

class _Key implements SecureKey {
  String? value = 'sk-ant-api03-shots';

  @override
  Future<String?> read() async => value;

  @override
  Future<void> write(String? key) async => value = key;
}

Widget _scoped(bool developer, Widget child) {
  final on = ValueNotifier(developer);
  addTearDown(on.dispose);
  return DeveloperScope(notifier: on, child: child);
}

void main() {
  for (final developer in [false, true]) {
    final mode = developer ? 'on' : 'off';

    shots('developer-settings-$mode', (s) async {
      final dir = Directory.systemTemp.createTempSync('clax-shots-dev-');
      addTearDown(() => dir.deleteSync(recursive: true));
      final store = SettingsStore(File('${dir.path}/settings.json'));
      await s.tester.runAsync(
        () => store.save(AppSettings(developer: developer)),
      );
      final at = DateTime.utc(2026, 10, 12, 9, 14);
      await s.tester.pumpWidget(
        _scoped(
          developer,
          s.app(
            SettingsScreen(
              store: store,
              secureKey: _Key(),
              cliSupported: s.form == mac,
              timings: [
                TimingRecord(
                  name: sayUsable,
                  elapsed: const Duration(milliseconds: 8400),
                  at: at,
                ),
                TimingRecord(
                  name: sendFirstStep,
                  elapsed: const Duration(milliseconds: 1200),
                  at: at,
                ),
                TimingRecord(
                  name: longpressLanded,
                  elapsed: const Duration(milliseconds: 21700),
                  at: at,
                ),
              ],
            ),
            title: 'Settings',
          ),
        ),
      );
      await s.tester.runAsync(() => Future<void>.delayed(Duration.zero));
      await s.settle();
      final list = s.tester.state<ScrollableState>(
        find
            .descendant(
              of: find.byType(CustomScrollView),
              matching: find.byType(Scrollable),
            )
            .first,
      );
      list.position.jumpTo(list.position.maxScrollExtent);
      await s.settle(2);
      await s.take('developer-settings-$mode');
    });

    for (final tab in ['Overview', if (developer) 'Files', 'Permissions']) {
      final surface = 'developer-inspector-${tab.toLowerCase()}-$mode';
      shots(surface, (s) async {
        final api = FakeClaxApi(clock: () => '2026-10-12T08:30:00Z');
        final aid = api.seedArtifact(
          'Run log',
          {
            'index.html':
                '<!doctype html>\n<title>Run log</title>\n'
                '<script type="module" src="app.js"></script>\n',
            'app.js': 'export const total = 42;\n',
            'icon.png': 'not text',
          },
          capabilities: {
            'db': {'read': <Object?>[]},
            'notify': <String, Object?>{},
          },
        );
        api.putDoc(aid, 'runs/r1', {'km': 5});
        await s.tester.pumpWidget(
          _scoped(
            developer,
            s.app(AppInspector(api: api, aid: aid), title: 'Run log'),
          ),
        );
        await s.settle();
        if (tab != 'Overview') {
          await s.tester.tap(find.widgetWithText(Tab, tab));
          await s.settle();
        }
        await s.take(surface);
      });
    }
  }
}
