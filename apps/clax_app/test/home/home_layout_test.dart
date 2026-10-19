import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/home/home_host.dart';
import 'package:clax_app/home/home_layout.dart';
import 'package:flutter_test/flutter_test.dart';

/// App [id], updated [hoursAgo] hours before 2026-10-16 12:00 UTC.
HomeApp _app(String id, [int hoursAgo = 0]) => HomeApp(
  id: id,
  title: id.toUpperCase(),
  updatedAt: DateTime.utc(2026, 10, 16, 12).subtract(Duration(hours: hoursAgo)),
  icon: const {},
);

final _a = _app('a', 1), _b = _app('b', 2), _c = _app('c', 3);
final _d = _app('d', 4), _e = _app('e', 5);

const _path = 'layout/main';

/// A fake whose next [races] layout writes are each overtaken by
/// [foreign], applied to the stored layout just before the write lands.
class _RacedApi extends FakeClaxApi {
  int races = 0;
  Map<String, Object?> Function(Map<String, Object?>) foreign = (d) => d;

  @override
  Future<void> batchDocs(String aid, List<DocWrite> writes) async {
    if (races > 0) {
      races--;
      final have = await super.getDoc(aid, _path);
      putDoc(aid, _path, foreign(have?.data ?? const {}));
    }
    return super.batchDocs(aid, writes);
  }
}

void main() {
  group('normalizeLayout', () {
    test('fills what is missing and keeps fields it does not know', () {
      expect(normalizeLayout(null), {
        'v': 1,
        'starred': <String>[],
        'order': <Object?>[],
        'groups': <String, Object?>{},
        'seen': <String>[],
        'opens': <String, Object?>{},
      });
      final n = normalizeLayout({
        'v': 1,
        'starred': ['a'],
        'order': ['b', 'g:x', 'zzz'],
        'groups': {
          'x': {
            'name': 'Tools',
            'look': 'pile',
            'apps': ['c', 'd'],
            'colour': 'teal',
          },
        },
        'seen': ['a'],
        'opens': <String, Object?>{},
        'bigTiles': false,
        'wallpaper': {'kind': 'dots'},
      });
      expect(n['bigTiles'], false);
      expect(n['wallpaper'], {'kind': 'dots'});
      expect((n['groups'] as Map)['x'], {
        'name': 'Tools',
        'look': 'pile',
        'apps': ['c', 'd'],
        'colour': 'teal',
      });
    });

    test('drops malformed entries and duplicates', () {
      final n = normalizeLayout({
        'starred': ['a', 'a', 3, ''],
        'order': 'b',
        'groups': {
          'x': {
            'apps': ['c'],
            'look': 7,
          },
          '': <String, Object?>{},
        },
        'opens': {
          'a': {
            'n': -1,
            'days': {'2026-10-16': 2, 'nope': 1},
          },
        },
      });
      expect(n['starred'], ['a']);
      expect(n['order'], isEmpty);
      expect(n['groups'], {
        'x': {
          'name': 'Group',
          'look': 'folder',
          'apps': ['c'],
        },
      });
      expect((n['opens'] as Map)['a'], {
        'n': 0,
        'last': null,
        'days': {'2026-10-16': 2},
      });
    });
  });

  group('reconcileLayout', () {
    test('a first load lists the apps newest first, every one seen', () {
      final l = reconcileLayout(null, [_b, _a, _c], first: true);
      expect(l['order'], ['a', 'b', 'c']);
      expect((l['seen'] as List).toSet(), {'a', 'b', 'c'});
    });

    test(
      'a listed app in no place goes first, unseen; an app no longer listed goes everywhere',
      () {
        final l = reconcileLayout(
          {
            'starred': ['a', 'gone'],
            'order': ['b', 'gone', 'c'],
            'seen': ['a', 'b', 'c', 'gone'],
            'opens': {
              'gone': {'n': 3, 'last': null, 'days': <String, Object?>{}},
            },
          },
          [_d, _a, _b, _c],
        );
        expect(l['starred'], ['a']);
        expect(l['order'], ['d', 'b', 'c']);
        expect(l['seen'], isNot(contains('d')));
        expect(l['seen'], isNot(contains('gone')));
        expect((l['opens'] as Map).containsKey('gone'), isFalse);
      },
    );

    test('an app in two places keeps Starred, then its group, then All apps', () {
      final l = reconcileLayout(
        {
          'starred': ['a'],
          'order': ['a', 'g:x', 'b', 'c'],
          'groups': {
            'x': {
              'name': 'X',
              'apps': ['a', 'b', 'c', 'd'],
            },
          },
          'seen': ['a', 'b', 'c', 'd'],
        },
        [_a, _b, _c, _d],
      );
      expect(l['starred'], ['a']);
      expect(((l['groups'] as Map)['x'] as Map)['apps'], ['b', 'c', 'd']);
      expect(l['order'], ['g:x']);
    });

    test(
      'a group left with one app gives way to it in place; one left with none goes',
      () {
        final l = reconcileLayout(
          {
            'order': ['a', 'g:x', 'g:y', 'b'],
            'groups': {
              'x': {
                'name': 'X',
                'apps': ['c', 'gone'],
              },
              'y': {
                'name': 'Y',
                'apps': ['gone2'],
              },
            },
            'seen': ['a', 'b', 'c'],
          },
          [_a, _b, _c],
        );
        expect(l['order'], ['a', 'c', 'b']);
        expect(l['groups'], isEmpty);
      },
    );

    test(
      'a group no entry names is put at the end of All apps; a g: entry with no group goes',
      () {
        final l = reconcileLayout(
          {
            'order': ['a', 'g:missing'],
            'groups': {
              'x': {
                'name': 'X',
                'apps': ['b', 'c'],
              },
            },
            'seen': ['a', 'b', 'c'],
          },
          [_a, _b, _c],
        );
        expect(l['order'], ['a', 'g:x']);
      },
    );

    test('keeps fields and groups an earlier release does not show', () {
      final l = reconcileLayout(
        {
          'v': 1,
          'order': ['g:x', 'a'],
          'groups': {
            'x': {
              'name': 'Tools',
              'look': 'pile',
              'apps': ['b', 'c'],
            },
          },
          'seen': ['a', 'b', 'c'],
          'bigTiles': false,
          'wallpaper': 'dots',
        },
        [_a, _b, _c],
      );
      expect(l['order'], ['g:x', 'a']);
      expect(((l['groups'] as Map)['x'] as Map)['look'], 'pile');
      expect(l['bigTiles'], false);
      expect(l['wallpaper'], 'dots');
      expect(l['seen'], contains('b'));
    });

    test('keeps order entries of kinds it does not know in place, unshown', () {
      final spacer = {'kind': 'spacer', 'size': 2};
      final later = {
        'order': ['a', 's:1', 'b', spacer, 'g:x', 's:1'],
        'groups': {
          'x': {
            'name': 'X',
            'apps': ['c', 'd'],
          },
        },
        'seen': ['a', 'b', 'c', 'd'],
      };
      final l = reconcileLayout(later, [_a, _b, _c, _d]);
      expect(l['order'], ['a', 's:1', 'b', spacer, 'g:x']);
      expect(layoutSections(l, [_a, _b, _c, _d]).order, ['a', 'b', 'g:x']);
      expect(reconcileLayout(later, [_a, _c, _d])['order'], [
        'a',
        's:1',
        spacer,
        'g:x',
      ]);
      // An index into All apps counts only the entries shown.
      expect(moveLayout(l, 'b', LayoutSection.all, 0)['order'], [
        'b',
        'a',
        's:1',
        spacer,
        'g:x',
      ]);
    });

    test('keeps a v above 1 and never lowers it', () {
      expect(normalizeLayout({'v': 2})['v'], 2);
      expect(normalizeLayout({'v': 2.5})['v'], 2.5);
      expect(reconcileLayout({'v': 3, 'order': ['a']}, [_a])['v'], 3);
      expect(keepNewerLayout({'v': 2}, {'v': 1, 'starred': <String>[]})['v'], 2);
    });

    test('a v above 1 writes back unchanged the known fields it cannot read', () {
      final stored = {
        'v': 2,
        'starred': {'a': 1},
        'order': 'by-use',
        'groups': ['x'],
        'seen': ['a'],
        'opens': 5,
        'bigTiles': 'auto',
        'note': 'n',
      };
      final next = reconcileLayout(stored, [_a, _b]);
      expect(keepNewerLayout(stored, next), {
        ...next,
        'starred': {'a': 1},
        'order': 'by-use',
        'groups': ['x'],
        'opens': 5,
        'bigTiles': 'auto',
      });
      final old = {...stored, 'v': 1};
      expect(keepNewerLayout(old, reconcileLayout(old, [_a, _b])), reconcileLayout(old, [_a, _b]));
      expect(keepNewerLayout(null, {'v': 1}), {'v': 1});
    });
  });

  group('moveLayout', () {
    Map<String, Object?> base() => reconcileLayout(
      {
        'starred': ['a'],
        'order': ['b', 'g:x', 'c'],
        'groups': {
          'x': {
            'name': 'X',
            'apps': ['d', 'e'],
          },
        },
        'seen': ['a', 'b', 'c', 'd', 'e'],
      },
      [_a, _b, _c, _d, _e],
    );

    test('moves within and between sections, clamped', () {
      expect(moveLayout(base(), 'c', LayoutSection.all, 0)['order'], [
        'c',
        'b',
        'g:x',
      ]);
      expect(moveLayout(base(), 'b', LayoutSection.all, 99)['order'], [
        'g:x',
        'c',
        'b',
      ]);
      final s = moveLayout(base(), 'c', LayoutSection.starred, 0);
      expect(s['starred'], ['c', 'a']);
      expect(s['order'], ['b', 'g:x']);
      expect(moveLayout(base(), 'a', LayoutSection.all, 1)['order'], [
        'b',
        'a',
        'g:x',
        'c',
      ]);
    });

    test('a group moves like an app but is never starred', () {
      expect(moveLayout(base(), 'g:x', LayoutSection.all, 0)['order'], [
        'g:x',
        'b',
        'c',
      ]);
      expect(moveLayout(base(), 'g:x', LayoutSection.starred, 0), base());
    });

    test('starring takes an app out of its group', () {
      final s = moveLayout(base(), 'd', LayoutSection.starred, null);
      expect(s['starred'], ['a', 'd']);
      expect(((s['groups'] as Map)['x'] as Map)['apps'], ['e']);
    });
  });

  group('HomeLayout', () {
    late _RacedApi api;
    late String home;
    late HomeLayout layout;
    final apps = [_a, _b, _c, _d, _e];

    Future<void> seed(Map<String, Object?> data) => api.batchDocs(home, [
      DocWrite(op: 'set', collection: 'layout', docId: 'main', data: data),
    ]);

    Future<Doc?> stored() => api.getDoc(home, _path);

    List<DocWrite> writes() => [
      for (final c in api.calls)
        if (c.name == 'batchDocs') ...(c.args['writes'] as List<DocWrite>),
    ];

    setUp(() {
      api = _RacedApi();
      home = api.seedArtifact('Home', const {
        'index.html': '<!doctype html><title>Home</title>',
      });
      layout = HomeLayout(api: api, homeAid: home);
      addTearDown(layout.dispose);
    });

    test('sections follow the stored layout, an unplaced app first', () async {
      await seed({
        'starred': ['b'],
        'order': ['a', 'g:t', 'c'],
        'groups': {
          't': {
            'name': 'Tools',
            'look': 'folder',
            'apps': ['d', 'e'],
          },
        },
        'seen': ['a', 'b', 'c', 'd', 'e'],
      });
      await layout.load(apps);
      expect(layout.readable, isTrue);
      final s = layout.sections;
      expect(s.starred, ['b']);
      expect(s.order, ['a', 'g:t', 'c']);
      expect(s.groups['t']!.name, 'Tools');
      expect(s.groups['t']!.look, 'folder');
      expect(s.groups['t']!.apps, ['d', 'e']);

      await seed2(api, home, {
        'starred': ['b'],
        'order': ['a', 'g:t', 'c'],
        'groups': {
          't': {
            'name': 'Tools',
            'look': 'stack',
            'apps': ['d'],
          },
        },
      });
      await layout.load(apps);
      // A group left with one app gives way to it; an unplaced app goes
      // first; an unknown look shows as a folder.
      expect(layout.sections.order, ['e', 'a', 'd', 'c']);
      expect(layout.sections.groups, isEmpty);
      expect(writes(), hasLength(2), reason: 'only the seeds write');
    });

    test('star writes with the version read and keeps what it does not know', () async {
      await seed({
        'v': 1,
        'starred': ['b'],
        'order': ['a', 'c', 'd', 'e'],
        'seen': ['a', 'b'],
        'opens': {
          'a': {
            'n': 2,
            'last': '2026-10-16T10:00:00.000Z',
            'days': {'2026-10-16': 2},
          },
          'gone': {'n': 1, 'last': null, 'days': <String, Object?>{}},
        },
        'big': ['a'],
        'bigTiles': false,
        'future': 1,
      });
      await layout.load(apps);
      final read = (await stored())!;
      await layout.star('a');
      final w = writes().last;
      expect(w.op, 'set');
      expect(w.path, _path);
      expect(w.ifVersion, read.version);
      final now = (await stored())!.data;
      expect(now['starred'], ['b', 'a']);
      expect(now['order'], ['c', 'd', 'e']);
      expect(now['opens'], read.data['opens']);
      expect(now['seen'], ['a', 'b']);
      expect(now['big'], ['a']);
      expect(now['bigTiles'], false);
      expect(now['future'], 1);
      expect(now['v'], 1);
      expect(layout.sections.starred, ['b', 'a']);
    });

    test('unstar and moveTo write the whole document', () async {
      await seed({
        'v': 2,
        'starred': ['b'],
        'order': ['a', 'c', 'd', 'e'],
        'later': 'kept',
      });
      await layout.load(apps);
      await layout.unstar('b');
      expect((await stored())!.data['order'], ['b', 'a', 'c', 'd', 'e']);
      await layout.moveTo('e', 1);
      final now = (await stored())!.data;
      expect(now['order'], ['b', 'e', 'a', 'c', 'd']);
      expect(now['starred'], isEmpty);
      expect(now['v'], 2);
      expect(now['later'], 'kept');
      expect(layout.sections.order, ['b', 'e', 'a', 'c', 'd']);
    });

    test('starAt puts an app at a place in Starred, out of All apps', () async {
      await seed({
        'starred': ['b'],
        'order': ['a', 'c', 'd', 'e'],
      });
      await layout.load(apps);
      await layout.starAt('d', 0);
      var now = (await stored())!.data;
      expect(now['starred'], ['d', 'b']);
      expect(now['order'], ['a', 'c', 'e']);
      await layout.starAt('a', 9);
      now = (await stored())!.data;
      expect(now['starred'], ['d', 'b', 'a']);
      expect(writes(), hasLength(3));
      expect(layout.sections.starred, ['d', 'b', 'a']);
    });

    test('a write overtaken by another re-reads and lands once, with both changes', () async {
      await seed({
        'starred': <String>[],
        'order': ['a', 'b', 'c', 'd', 'e'],
        'opens': {
          'a': {'n': 1, 'last': null, 'days': <String, Object?>{}},
        },
      });
      await layout.load(apps);
      api.races = 1;
      api.foreign = (d) => {
        ...d,
        'starred': ['c'],
        'order': ['a', 'b', 'd', 'e'],
        'opens': {
          'a': {'n': 2, 'last': null, 'days': <String, Object?>{}},
        },
      };
      await layout.star('a');
      final now = (await stored())!.data;
      expect(now['starred'], ['c', 'a']);
      expect(now['order'], ['b', 'd', 'e']);
      expect((now['opens'] as Map)['a'], {'n': 2, 'last': null, 'days': <String, Object?>{}});
      expect(writes(), hasLength(3), reason: 'the seed, the refused write, the retry');
      expect(layout.sections.starred, ['c', 'a']);
    });

    test('a second race gives up without writing and shows the stored layout', () async {
      await seed({
        'starred': <String>[],
        'order': ['a', 'b', 'c', 'd', 'e'],
      });
      await layout.load(apps);
      api.races = 2;
      api.foreign = (d) => {
        ...d,
        'starred': ['c'],
        'order': ['a', 'b', 'd', 'e'],
      };
      await expectLater(
        layout.star('a'),
        throwsA(isA<ClaxApiError>().having((e) => e.isConflict, 'conflict', isTrue)),
      );
      expect((await stored())!.data['starred'], ['c']);
      expect(layout.sections.starred, ['c']);
      expect(layout.sections.order, ['a', 'b', 'd', 'e']);
    });

    test('a missing layout lists apps newest first and writes nothing', () async {
      await layout.load([_c, _a, _e, _b, _d]);
      expect(layout.readable, isFalse);
      expect(layout.sections.starred, isEmpty);
      expect(layout.sections.order, ['a', 'b', 'c', 'd', 'e']);
      await layout.star('a');
      await layout.moveTo('e', 0);
      expect(writes(), isEmpty);
      expect(await stored(), isNull);
    });

    test('a layout whose order is a string is unreadable and never written', () async {
      await seed({
        'starred': ['b'],
        'order': 'by-use',
      });
      await layout.load(apps);
      expect(layout.readable, isFalse);
      expect(layout.sections.order, ['a', 'b', 'c', 'd', 'e']);
      expect(layout.sections.starred, isEmpty);
      await layout.star('a');
      expect(writes(), hasLength(1));
      expect((await stored())!.data['order'], 'by-use');
    });

    test('an unreadable home app reads as unreadable', () async {
      final gone = HomeLayout(api: api, homeAid: 'nope');
      addTearDown(gone.dispose);
      await gone.load(apps);
      expect(gone.readable, isFalse);
      expect(gone.sections.order, ['a', 'b', 'c', 'd', 'e']);
    });
  });
}

/// Replaces the stored layout as another writer would.
Future<void> seed2(FakeClaxApi api, String home, Map<String, Object?> data) async {
  final have = await api.getDoc(home, _path);
  await api.batchDocs(home, [
    DocWrite(
      op: 'set',
      collection: 'layout',
      docId: 'main',
      data: data,
      ifVersion: have?.version,
    ),
  ]);
}
