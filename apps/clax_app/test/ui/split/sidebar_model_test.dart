import 'dart:async';
import 'dart:ui' show Rect;

import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/home/home_host.dart';
import 'package:clax_app/home/home_settings.dart';
import 'package:clax_app/ui/make_service.dart';
import 'package:clax_app/ui/split/sidebar_model.dart';
import 'package:flutter_test/flutter_test.dart';

const _page = {'index.html': '<!doctype html><title>x</title>'};

class _Actions implements HomeActions {
  @override
  void open(String id) {}
  @override
  void openMake(String id) {}
  @override
  void newApp() {}
  @override
  Future<String?> menu(String id, Rect anchor, List<HomeMenuItem> items) async =>
      null;
  @override
  void openInbox() {}
  @override
  void openSettings() {}
  @override
  void importApp() {}
}

/// Each row as a line: section heads in capitals, a group's apps
/// indented.
List<String> describe(List<SidebarRow> rows) => [
  for (final r in rows)
    switch (r) {
      HomeRow() => 'Home',
      SectionRow(:final title) => title.toUpperCase(),
      AppRow(:final app, :final group) =>
        '${group != null ? '  ' : ''}${app.title}',
      GroupRow(:final group, :final expanded, :final count) =>
        '${expanded ? '▾' : '▸'} ${group.name} ($count)',
      MakeRow(:final make) => '◌ ${make.request}',
    },
];

void main() {
  late FakeClaxApi api;
  late HomeHost host;
  late MakeService makes;
  late SidebarModel model;
  late String home;
  final ids = <String, String>{};

  Future<void> settle() async {
    for (var i = 0; i < 5; i++) {
      await pumpEventQueue();
    }
  }

  setUp(() async {
    var t = 0;
    api = FakeClaxApi(
      clock: () => '2026-10-12T00:00:${(t++).toString().padLeft(2, '0')}Z',
    );
    home = api.seedArtifact('Home', _page);
    for (final title in [
      'Runs',
      'Notes',
      'Groceries',
      'Budget',
      'Weather',
      'Hammer',
      'Saw',
    ]) {
      ids[title] = api.seedArtifact(title, _page);
    }
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
          'opens': {
            ids['Runs']: {'n': 3, 'last': null, 'days': <String, Object?>{}},
          },
        },
      ),
    ]);
    makes = MakeService.local((r, cancel, report) => Completer<String>().future);
    host = HomeHost(
      api: api,
      stream: api.stream(),
      closeStream: true,
      makes: makes,
      builds: null,
      settings: HomeSettings(homeAppId: home),
      actions: _Actions(),
    );
    makes.start('A tide table');
    model = SidebarModel(host: host);
    await settle();
  });

  tearDown(() {
    model.dispose();
    host.dispose();
  });

  test('rows: Home, Starred, Apps in the layout\'s order with groups as '
      'disclosure rows, and Making', () {
    expect(describe(model.rows), [
      'Home',
      'STARRED',
      'Runs',
      'Notes',
      'APPS',
      'Groceries',
      '▸ Tools (2)',
      'Budget',
      'Weather',
      'MAKING',
      '◌ A tide table',
    ]);
    expect(model.selection, const HomeSelected());
  });

  test('a group opens and closes', () {
    model.toggleGroup('tools');
    expect(describe(model.rows), contains('  Hammer'));
    expect(describe(model.rows), contains('▾ Tools (2)'));
    model.toggleGroup('tools');
    expect(describe(model.rows), isNot(contains('  Hammer')));
  });

  test('without stars or makes their sections do not show', () async {
    await api.batchDocs(home, [
      DocWrite(
        op: 'set',
        collection: 'layout',
        docId: 'main',
        data: {'order': <String>[]},
        ifVersion: (await api.getDoc(home, 'layout/main'))!.version,
      ),
    ]);
    makes.dismiss(makes.makes.single);
    await host.layout!.load(host.state.apps);
    await settle();
    final rows = describe(model.rows);
    expect(rows.first, 'Home');
    expect(rows, isNot(contains('STARRED')));
    expect(rows, isNot(contains('MAKING')));
    expect(rows, contains('APPS'));
    // Newest first, as nothing places them.
    expect(rows[2], 'Saw');
  });

  test('a selected make no longer listed gives way to Home, unless the '
      'model keeps it', () async {
    final make = MakeSelected(host.state.makes.single.id);
    model.select(make);
    makes.dismiss(makes.makes.single);
    await settle();
    expect(model.selection, const HomeSelected());

    makes.start('A tide table');
    await settle();
    final again = MakeSelected(host.state.makes.single.id);
    final kept = SidebarModel(host: host, keeps: (s) => s == again);
    addTearDown(kept.dispose);
    kept.select(again);
    makes.dismiss(makes.makes.single);
    await settle();
    expect(kept.selection, again);
  });

  test('typeSelect selects by title; an app in a closed group opens it', () {
    expect(model.typeSelect('gr'), isTrue);
    expect(model.selection, AppSelected(ids['Groceries']!));
    expect(model.typeSelect('SA'), isTrue);
    expect(model.selection, AppSelected(ids['Saw']!));
    expect(describe(model.rows), contains('  Saw'));
    expect(model.typeSelect('zzz'), isFalse);
    expect(model.selection, AppSelected(ids['Saw']!));
  });

  test('moveSelection steps over section heads and group rows', () {
    model.moveSelection(1);
    expect(model.selection, AppSelected(ids['Runs']!));
    model.moveSelection(2);
    expect(model.selection, AppSelected(ids['Groceries']!));
    model.moveSelection(1);
    expect(model.selection, AppSelected(ids['Budget']!));
    model.moveSelection(3);
    expect(model.selection, MakeSelected(host.state.makes.single.id));
    model.moveSelection(5);
    expect(model.selection, MakeSelected(host.state.makes.single.id));
    model.moveSelection(-100);
    expect(model.selection, const HomeSelected());
  });

  test('drop into Starred stars the app at that place, in one pinned write', () async {
    final before = (await api.getDoc(home, 'layout/main'))!;
    await model.drop(ids['Budget']!, index: 0, toStarred: true);
    await settle();
    final now = (await api.getDoc(home, 'layout/main'))!;
    expect(now.version, before.version + 1);
    expect(now.data['starred'], [ids['Budget'], ids['Runs'], ids['Notes']]);
    expect(now.data['opens'], before.data['opens']);
    expect(describe(model.rows).sublist(1, 5), [
      'STARRED',
      'Budget',
      'Runs',
      'Notes',
    ]);
  });

  test('drop into Apps moves the app there, out of Starred', () async {
    await model.drop(ids['Runs']!, index: 1);
    await settle();
    final rows = describe(model.rows);
    expect(rows.sublist(1, 4), ['STARRED', 'Notes', 'APPS']);
    expect(rows.sublist(4, 7), ['Groceries', 'Runs', '▸ Tools (2)']);
  });

  test('a filter leaves only the matching rows', () {
    model.filter = 'tid';
    expect(describe(model.rows), ['MAKING', '◌ A tide table']);
    model.filter = 'a';
    final rows = describe(model.rows);
    // A group with a match opens, showing only what matches.
    expect(rows, contains('▾ Tools (2)'));
    expect(rows, contains('  Hammer'));
    expect(rows, contains('  Saw'));
    expect(rows, isNot(contains('Runs')));
    expect(rows, isNot(contains('Home')));
    model.filter = '';
    expect(describe(model.rows).first, 'Home');
  });

  test('a selected app that goes away selects Home', () async {
    model.select(AppSelected(ids['Weather']!));
    await api.deleteArtifact(ids['Weather']!);
    await host.load();
    await settle();
    expect(model.selection, const HomeSelected());
  });

  test('the unread count of each app comes from unreadOf', () async {
    final counted = SidebarModel(
      host: host,
      unreadOf: (id) => id == ids['Notes'] ? 3 : 0,
    );
    addTearDown(counted.dispose);
    final notes = counted.rows.whereType<AppRow>().firstWhere(
      (r) => r.app.id == ids['Notes'],
    );
    expect(notes.unread, 3);
  });
}
