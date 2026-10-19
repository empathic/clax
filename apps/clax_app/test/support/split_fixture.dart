// A home screen's sources for the split layout's tests and renders: a
// FakeClaxApi with seven apps, two of them starred and two in a group, a
// make going on, and the HomeHost over them.
import 'dart:async';
import 'dart:ui' show Rect;

import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/home/home_host.dart';
import 'package:clax_app/home/home_settings.dart';
import 'package:clax_app/ui/make_service.dart';
import 'package:clax_app/ui/making_screen.dart' show MakeUpdate;

const _page = {'index.html': '<!doctype html><title>x</title>'};

/// The home screen's flows, recorded.
class RecordedActions implements HomeActions {
  final calls = <(String, List<Object?>)>[];
  @override
  void open(String id) => calls.add(('open', [id]));
  @override
  void openMake(String id) => calls.add(('openMake', [id]));
  @override
  void newApp() => calls.add(('newApp', []));
  @override
  Future<String?> menu(String id, Rect anchor, List<HomeMenuItem> items) async {
    calls.add(('menu', [id, anchor]));
    return null;
  }

  @override
  void openInbox() {}
  @override
  void openSettings() {}
  @override
  void importApp() => calls.add(('importApp', []));
}

/// The fixture: [ids] by title, and the host once [start]ed.
class SplitFixture {
  SplitFixture() {
    var t = 0;
    api = FakeClaxApi(
      clock: () => '2026-10-12T00:00:${(t++).toString().padLeft(2, '0')}Z',
    );
    home = api.seedArtifact('Home', _page);
    for (final title in titles) {
      ids[title] = api.seedArtifact(title, _page);
    }
  }

  static const titles = [
    'Runs',
    'Notes',
    'Groceries',
    'Budget',
    'Weather',
    'Hammer',
    'Saw',
  ];

  late final FakeClaxApi api;
  late final String home;
  final ids = <String, String>{};
  final actions = RecordedActions();
  late final MakeService makes = MakeService((r, cancel, report) {
    this.report = report;
    return Completer<String>().future;
  });

  /// What the make reports to its build view, once it started.
  void Function(MakeUpdate update)? report;
  late final HomeHost host;

  /// Arranges the apps (Runs and Notes starred; Groceries, the Tools group
  /// of Hammer and Saw, Budget and Weather in Apps), starts a make when
  /// [making], and makes the host.
  Future<void> start({bool making = true}) async {
    await api.batchDocs(home, [
      DocWrite(
        op: 'set',
        collection: 'layout',
        docId: 'main',
        data: {
          'starred': [ids['Runs'], ids['Notes']],
          'order': [ids['Groceries'], 'g:tools', ids['Budget'], ids['Weather']],
          'groups': {
            'tools': {
              'name': 'Tools',
              'look': 'folder',
              'apps': [ids['Hammer'], ids['Saw']],
            },
          },
          'seen': ids.values.toList(),
        },
      ),
    ]);
    host = HomeHost(
      api: api,
      stream: api.stream(),
      closeStream: true,
      makes: makes,
      builds: null,
      settings: HomeSettings(homeAppId: home),
      actions: actions,
    );
    if (making) makes.start('A tide table');
  }

  void dispose() => host.dispose();
}
