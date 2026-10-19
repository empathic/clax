// Existing screens in the brand's theme, rendered in every form and theme
// into CLAX_SHOTS_DIR (skipped without it): Settings with a key in its
// field, the launcher with apps, icons and makes, a thread, and the splash
// at rest and once it breathes.
import 'dart:async';
import 'dart:io';

import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/platform/secure_key.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_icon.dart';
import 'package:clax_app/ui/launcher.dart';
import 'package:clax_app/ui/make_service.dart';
import 'package:clax_app/ui/settings_screen.dart';
import 'package:clax_app/ui/splash.dart';
import 'package:clax_app/ui/thread_detail.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../support/fake_voice.dart';
import '../../support/paths.dart';
import 'rig.dart';

class _Key implements SecureKey {
  _Key(this.value);
  String? value;

  @override
  Future<String?> read() async => value;

  @override
  Future<void> write(String? key) async => value = key;
}

const _icons = {
  'Plant care': 'plants',
  'Weekly budget': 'budget',
  'Recipes': 'recipes',
  'Habit streaks': 'habits',
  'Groceries': 'groceries',
  'Run tracker': 'runs',
};

void main() {
  shots('settings', (s) async {
    final dir = Directory.systemTemp.createTempSync('clax-shots-settings-');
    addTearDown(() => dir.deleteSync(recursive: true));
    final store = SettingsStore(File('${dir.path}/settings.json'));
    await s.tester.runAsync(store.load);
    await s.tester.pumpWidget(
      s.app(
        SettingsScreen(
          store: store,
          secureKey: _Key('sk-ant-api03-shots'),
          cliSupported: s.form == mac,
          timings: const [],
        ),
        title: 'Settings',
      ),
    );
    await s.tester.runAsync(() => Future<void>.delayed(Duration.zero));
    await s.settle();
    await s.take('settings');
  });

  shots('launcher', (s) async {
    final makes = MakeService(
      (r, cancelled, stage) => Completer<String>().future,
      maxRunning: 1,
    );
    addTearDown(makes.dispose);
    makes.lost('A water reminder every two hours');
    makes.add('A reading log');
    makes.start('A packing list for trips');
    var t = 0;
    final api = FakeClaxApi(
      clock: () => '2026-10-12T00:00:${(t++).toString().padLeft(2, '0')}Z',
    );
    api.seedArtifact('Notes', {'index.html': '<p>n</p>'});
    api.seedArtifact('Mood journal', {'index.html': '<p>m</p>'});
    for (final e in _icons.entries) {
      api.seedArtifact(e.key, {
        'index.html': '<p>x</p>',
        'icon.svg': File(
          appPath('test/fixtures/icons/${e.value}.svg'),
        ).readAsStringSync(),
      });
    }
    await s.tester.pumpWidget(
      s.app(
        Launcher(
          api: api,
          open: (_) {},
          make: (r, cancelled, stage) => throw StateError('unused'),
          voice: FakeVoice.unsupported('No speech here'),
          icons: AppIcons(api),
          makes: makes,
        ),
      ),
    );
    // The making ring spins, so pump a while rather than settle.
    await s.settle(10);
    await s.take('launcher');
  });

  shots('thread', (s) async {
    final api = FakeClaxApi(clock: () => '2026-10-12T09:14:00Z');
    final aid = api.seedArtifact('Runs', {'index.html': '<h1>Runs</h1>'});
    await api.registerAppSession(installId: 'install', pid: 1);
    final item = api.sendComment(
      aid,
      'make the title blue',
      anchor: const {'kind': 'text', 'file': 'index.html', 'quote': 'Runs'},
    );
    await api.replyAsAgent(aid, item.threadId, 'Done: it is blue.');
    await api.postOwnerComment(aid, item.threadId, 'A little darker, please');
    await s.tester.pumpWidget(
      s.app(
        ThreadDetail(
          api: api,
          aid: aid,
          tid: item.threadId,
          voice: FakeVoice(),
          autoDictate: false,
          onPreview: (_) {},
          onRetry: (_) async => true,
          now: () => DateTime.utc(2026, 10, 12, 9, 20),
        ),
        title: 'Runs',
      ),
    );
    await s.settle();
    await s.take('thread');
  });

  shots('splash', (s) async {
    await s.tester.pumpWidget(s.app(const ClaxSplash()));
    await s.take('splash');
    // Past the breath, so nothing is left waiting when the test ends.
    await s.tester.pumpWidget(const SizedBox());
    await s.tester.pump(const Duration(milliseconds: 600));
  });

  shots('splash-starting', (s) async {
    await s.tester.pumpWidget(s.app(const ClaxSplash()));
    // 600 ms to the breath, then frames to 700 ms: the dot on its way
    // toward Clax, "Starting…" a third faded in.
    await s.tester.pump(const Duration(milliseconds: 600));
    for (var i = 0; i < 4; i++) {
      await s.tester.pump(const Duration(milliseconds: 25));
    }
    await s.take('splash-starting');
    await s.tester.pumpWidget(const SizedBox());
  });
}
