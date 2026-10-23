// Combining versions and the sync badges, rendered in every form and theme
// into CLAX_SHOTS_DIR (skipped without it): the diverged sheet, the update
// sheet of a modified copy with Bring v3 into mine, its offer "Your
// version with v3", a failed check with Tell @owner and its composer, an
// author's update to a version made on the web, and the launcher's tiles
// with each sync badge.
import 'dart:io';

import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/remote/checker.dart' show LinkedStatus;
import 'package:clax_app/remote/sync_state.dart';
import 'package:clax_app/ui/app_icon.dart';
import 'package:clax_app/ui/bundle_sheets.dart';
import 'package:clax_app/ui/launcher.dart';
import 'package:clax_app/ui/remote/diverged_sheet.dart';
import 'package:clax_app/ui/remote/publish_sheet.dart';
import 'package:clax_app/ui/working_chip.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../support/fake_voice.dart';
import '../../support/paths.dart';
import 'rig.dart';

const _bundle = BundleSummary(
  lineage: 'l_aaaaaaaaaaaaaaaaaaaaaaaaaa',
  title: 'Plant log',
  version: 3,
  fingerprint: '7KQ2-M9XD-4FTA-QW3B',
  capabilities: {'db': {}},
  word: 'leaf',
  byline: 'v3 by @sam · Plant log by @sam',
);

const _icons = {
  'Plant care': ('plants', SyncState.live),
  'Weekly budget': ('budget', SyncState.ahead),
  'Recipes': ('recipes', SyncState.updateWaiting),
  'Habit streaks': ('habits', SyncState.diverged),
  'Tide times': ('plants', SyncState.forked),
  'Groceries': ('groceries', SyncState.keptMine),
  'Run tracker': ('runs', SyncState.onlyHere),
};

TellOwner _tell() => TellOwner(
  owner: '@sam',
  draft: tellOwnerDraft(3, 'its tests failed: 2 of 9'),
  post: (_) async => null,
);

/// Shows [sheet] over a page, then [then] (a tap) before the shot.
Future<void> _sheet(
  Shot s,
  String surface,
  Widget sheet, {
  Future<void> Function(WidgetTester tester)? then,
}) async {
  final page = GlobalKey();
  await s.tester.pumpWidget(
    s.app(
      Scaffold(
        key: page,
        appBar: AppBar(title: const Text('Plant log')),
        body: const Center(child: Text('A page')),
      ),
    ),
  );
  showBundleSheet<Object?>(page.currentContext!, sheet);
  await s.settle();
  if (then != null) {
    await then(s.tester);
    await s.settle(3);
  }
  await s.take(surface);
}

void main() {
  shots(
    'remote-diverged',
    (s) => _sheet(
      s,
      'remote-diverged',
      const DivergedSheet(
        title: 'Plant log',
        host: 'clax.dev',
        who: '@sam',
        headVersion: 8,
        baseVersion: 7,
        canCombine: true,
        word: 'leaf',
      ),
    ),
  );
  shots(
    'remote-web-version',
    (s) => _sheet(
      s,
      'remote-web-version',
      const WebVersionSheet(
        title: 'Plant log',
        host: 'clax.dev',
        who: '@sam',
        version: 8,
        word: 'leaf',
      ),
    ),
    forms: const [phone],
  );
  shots(
    'bundle-update-bring',
    (s) => _sheet(
      s,
      'bundle-update-bring',
      const UpdateSheet(
        bundle: _bundle,
        have: 2,
        added: [],
        modified: true,
        canBringIntoMine: true,
        autoUpdate: false,
      ),
    ),
  );
  shots(
    'bundle-merged-offer',
    (s) => _sheet(
      s,
      'bundle-merged-offer',
      const MergedOfferSheet(
        version: 3,
        note:
            'Kept your watering schedule and the notes column, and took '
            'v3’s new sun chart.',
      ),
    ),
  );
  shots(
    'bundle-update-tell',
    (s) => _sheet(
      s,
      'bundle-update-tell',
      BundleProgressSheet(
        progress: ValueNotifier(
          updateCheckFailed(3, 'its tests failed: 2 of 9', tell: _tell()),
        ),
      ),
    ),
  );
  shots(
    'bundle-tell-owner',
    (s) => _sheet(s, 'bundle-tell-owner', TellOwnerSheet(tell: _tell())),
    forms: const [phone, mac],
  );
  shots(
    'remote-publish-fork',
    (s) => _sheet(
      s,
      'remote-publish-fork',
      const PublishSheet(
        title: 'Plant log',
        host: 'clax.dev',
        login: 'alex',
        version: 3,
        fileCount: 4,
        fileBytes: 12000,
        dataAllowed: true,
        forkName: 'Plant log (yours)',
        word: 'leaf',
      ),
    ),
    forms: const [phone, wide],
  );
  shots('pill-diverged', (s) async {
    await s.tester.pumpWidget(
      s.app(
        Scaffold(
          appBar: AppBar(title: const Text('Plant log')),
          body: const Stack(
            children: [
              Center(child: Text('A page')),
              Positioned(
                top: 8,
                left: 0,
                right: 0,
                child: Center(
                  child: ClaxPill(state: ClaxPillState.diverged),
                ),
              ),
            ],
          ),
        ),
      ),
    );
    await s.settle();
    await s.take('pill-diverged');
    // The pill's fold timer.
    await s.tester.pump(const Duration(seconds: 10));
  }, forms: const [phone, wide]);
  shots('launcher-sync', (s) async {
    final api = FakeClaxApi(clock: () => '2026-10-12T00:00:00Z');
    final linked = ValueNotifier<Map<String, LinkedStatus>>({});
    for (final MapEntry(key: title, value: (icon, state)) in _icons.entries) {
      final aid = api.seedArtifact(title, {
        'index.html': '<p>x</p>',
        'icon.svg': File(
          appPath('test/fixtures/icons/$icon.svg'),
        ).readAsStringSync(),
      });
      linked.value = {...linked.value, aid: LinkedStatus(state: state)};
    }
    await s.tester.pumpWidget(
      s.app(
        Launcher(
          api: api,
          open: (_) {},
          make: (r, cancelled, stage) => throw StateError('unused'),
          voice: FakeVoice.unsupported('No speech here'),
          icons: AppIcons(api, clean: cleanIconsHere),
          linked: linked,
        ),
      ),
    );
    await s.settle();
    await s.take('launcher-sync');
  });
}
