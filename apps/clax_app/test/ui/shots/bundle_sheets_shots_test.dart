// The bundle sheets rendered in every form and theme into CLAX_SHOTS_DIR
// (skipped without it), the phone's update sheet also at 200%, the
// install sheet also on an iPhone at 100% and 200%: export
// (own and a modified copy), install, update (as received and changed),
// the update check in progress, its failure with Details open, and a
// refused file.
import 'package:clax_app/bundle/exporter.dart' show ExportMode, ExportPlan;
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/bundle/updater.dart' show UpdateStep;
import 'package:clax_app/ui/bundle_sheets.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'rig.dart';

ExportPlan _plan(List<ExportMode> modes, int version) => ExportPlan(
  aid: 'a_plants',
  title: 'Plant log',
  iconWord: 'leaf',
  modes: modes,
  version: version,
  fileCount: 5,
  fileBytes: 20000,
  docCount: 3,
  dataAllowed: true,
  modified: modes.contains(ExportMode.fork),
  entry: modes.contains(ExportMode.own)
      ? null
      : LineageEntry(
          lineage: 'l_aaaaaaaaaaaaaaaaaaaaaaaaaa',
          role: LineageRole.installed,
          key: 'A' * 43,
          name: 'Alex',
          version: 2,
          released: const ['sha256:x'],
          capabilities: const {},
          title: 'Plant log',
        ),
);

const _bundle = BundleSummary(
  lineage: 'l_aaaaaaaaaaaaaaaaaaaaaaaaaa',
  title: 'Plant log',
  version: 3,
  fingerprint: '7KQ2-M9XD-4FTA-QW3B',
  name: 'Alex',
  capabilities: {
    'db': {},
    'notify': {},
    'net': {
      'hosts': ['api.weather.example'],
    },
  },
  word: 'leaf',
  seedDocs: 12,
);

/// Shows [sheet] over a page, then [then] (a tap) before the shot.
Future<void> _sheet(
  Shot s,
  String surface,
  Widget sheet, {
  bool dismissible = true,
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
  showBundleSheet<Object?>(
    page.currentContext!,
    sheet,
    dismissible: dismissible,
  );
  await s.settle();
  if (then != null) {
    await then(s.tester);
    await s.settle(3);
  }
  await s.take(surface);
}

void main() {
  shots(
    'bundle-export',
    (s) => _sheet(
      s,
      'bundle-export',
      ExportSheet(
        plan: _plan(const [ExportMode.own], 3),
        askName: true,
        toFile: s.form == mac,
      ),
    ),
  );
  shots(
    'bundle-export-modified',
    (s) => _sheet(
      s,
      'bundle-export-modified',
      ExportSheet(
        plan: _plan(const [ExportMode.reshare, ExportMode.fork], 1),
        toFile: s.form == mac,
      ),
    ),
  );
  shots(
    'bundle-install',
    (s) => _sheet(
      s,
      'bundle-install',
      const InstallSheet(bundle: _bundle, kind: InstallKind.install),
    ),
  );
  shots(
    'bundle-install',
    (s) => _sheet(
      s,
      'bundle-install',
      const InstallSheet(bundle: _bundle, kind: InstallKind.install),
    ),
    forms: const [iphone],
    textScales: const [1, 2],
  );
  shots(
    'bundle-update',
    (s) => _sheet(
      s,
      'bundle-update',
      const UpdateSheet(
        bundle: _bundle,
        have: 2,
        added: ['take photos and pick photos from your library'],
      ),
    ),
  );
  shots(
    'bundle-update-modified',
    (s) => _sheet(
      s,
      'bundle-update-modified',
      UpdateSheet(
        bundle: _bundle,
        have: 2,
        added: const [],
        modified: true,
        stoppedAt: DateTime(2026, 10, 2),
      ),
    ),
    textScales: const [1, 2],
  );
  shots(
    'bundle-checking',
    (s) => _sheet(
      s,
      'bundle-checking',
      BundleProgressSheet(
        progress: ValueNotifier(
          BundleWorking(
            'Checking v3 against your data…',
            updateStepLabel(UpdateStep.tests),
          ),
        ),
        onCancel: () {},
      ),
      dismissible: false,
    ),
  );
  shots(
    'bundle-failed',
    (s) => _sheet(
      s,
      'bundle-failed',
      BundleProgressSheet(
        progress: ValueNotifier(
          updateCheckFailed(
            3,
            'a check failed',
            details: ['adds a plant: expected 1, got 0'],
          ),
        ),
      ),
      then: (t) => t.tap(find.byKey(const ValueKey('details-toggle'))),
    ),
  );
  shots(
    'bundle-refused',
    (s) => _sheet(
      s,
      'bundle-refused',
      BundleNoticeSheet.refused('bad_signature'),
    ),
  );
}
