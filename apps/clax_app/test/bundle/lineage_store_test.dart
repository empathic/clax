import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';

import 'package:clax_app/bundle/lineage_store.dart';
import 'package:flutter_test/flutter_test.dart';

const _key = 'd1qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo';
const _h1 =
    'sha256:1111111111111111111111111111111111111111111111111111111111111111';
const _h2 =
    'sha256:2222222222222222222222222222222222222222222222222222222222222222';

LineageEntry _author({int version = 1, List<String> released = const [_h1]}) =>
    LineageEntry(
      lineage: 'l_aaaaaaaaaaaaaaaaaaaaaaaaaa',
      role: LineageRole.author,
      key: _key,
      name: 'Alex',
      version: version,
      released: released,
      capabilities: const {
        'db': {'write': true},
      },
      title: 'Plant log',
    );

void main() {
  late Directory dir;
  setUp(() => dir = Directory.systemTemp.createTempSync('lineages'));
  tearDown(() => dir.deleteSync(recursive: true));

  test('a missing or unreadable file is an empty store', () async {
    final store = LineageStore(dir);
    expect(await store.all(), isEmpty);
    File('${dir.path}/lineages.json').writeAsStringSync('{not json');
    expect(await store.all(), isEmpty);
    expect(await store.entry('a1'), isNull);
  });

  test('puts round-trip through the file in the spec shape', () async {
    final store = LineageStore(dir);
    final entry = _author().copyWith(
      upstream: const LineageUpstream(
        lineage: 'l_bbbbbbbbbbbbbbbbbbbbbbbbbb',
        key: _key,
      ),
      fromBundle: true,
    );
    await store.put('a1', entry);

    final raw = jsonDecode(
      File('${dir.path}/lineages.json').readAsStringSync(),
    ) as Map<String, Object?>;
    expect(raw['a1'], {
      'lineage': 'l_aaaaaaaaaaaaaaaaaaaaaaaaaa',
      'role': 'author',
      'key': _key,
      'name': 'Alex',
      'version': 1,
      'released': [_h1],
      'capabilities': {
        'db': {'write': true},
      },
      'title': 'Plant log',
      'updates': 'on',
      'stopped_at': null,
      'upstream': {'lineage': 'l_bbbbbbbbbbbbbbbbbbbbbbbbbb', 'key': _key},
      'from_bundle': true,
    });
    expect(File('${dir.path}/lineages.json.tmp').existsSync(), isFalse);

    final back = (await LineageStore(dir).entry('a1'))!;
    expect(back.toJson(), entry.toJson());
  });

  test(
    'stopping updates keeps the time; forget drops entry and bundle',
    () async {
      final at = DateTime.utc(2026, 10, 13, 12);
      final store = LineageStore(dir, now: () => at);
      await store.put('a1', _author().copyWith(role: LineageRole.installed));
      await store.put('a2', _author());
      await store.keepBundle('a1', Uint8List.fromList([1, 2, 3]));
      expect(await store.keptBundle('a1'), [1, 2, 3]);
      expect(await store.keptBundle('a2'), isNull);

      await store.stopUpdates('a1');
      final stopped = (await store.entry('a1'))!;
      expect(stopped.updatesStopped, isTrue);
      expect(stopped.stoppedAt, at);

      await store.forget('a1');
      expect(await store.entry('a1'), isNull);
      expect(await store.entry('a2'), isNotNull);
      expect(await store.keptBundle('a1'), isNull);
      expect(store.bundleFile('a1').existsSync(), isFalse);
      // Forgetting an app with no entry is not an error.
      await store.forget('nope');
    },
  );

  test('a malformed file is set aside before a write, and reported', () async {
    final at = DateTime.utc(2026, 10, 14, 1, 2, 3);
    final store = LineageStore(dir, now: () => at);
    File('${dir.path}/lineages.json').writeAsStringSync('{not json');
    expect(await store.all(), isEmpty);
    expect(store.damaged, isTrue);
    expect(store.setAside, isNull);

    await store.put('a2', _author());
    final aside = store.setAside!;
    expect(
      aside.path,
      '${dir.path}/lineages.json.corrupt-${at.millisecondsSinceEpoch}',
    );
    expect(aside.readAsStringSync(), '{not json');
    expect(store.damaged, isFalse);
    expect((await store.all()).keys, ['a2']);
  });

  test('a forget during an update stays forgotten', () async {
    final store = LineageStore(dir);
    await store.put('a1', _author());
    final gate = Completer<void>();
    final updating = store.update('a1', (e) async {
      await gate.future;
      return e?.copyWith(version: 5);
    });
    final forgetting = store.forget('a1');
    gate.complete();
    await updating;
    await forgetting;
    expect(await store.entry('a1'), isNull);

    // A stale entry read before the forget is not written back either,
    // nor is a bundle kept for the forgotten app.
    await store.put('a3', _author());
    final stale = (await store.entry('a3'))!;
    await store.forget('a3');
    await store.put('a3', stale.copyWith(version: 2));
    await store.keepBundle('a3', Uint8List.fromList([1]));
    await store.stopUpdates('a3');
    expect(await store.entry('a3'), isNull);
    expect(await store.keptBundle('a3'), isNull);
  });

  test('update re-reads the entry and may leave it', () async {
    final store = LineageStore(dir);
    await store.put('a1', _author());
    await store.update('a1', (e) async => e!.copyWith(version: 2));
    expect((await store.entry('a1'))!.version, 2);
    await store.update('a1', (e) async => null);
    expect((await store.entry('a1'))!.version, 2);
    await store.update('nope', (e) async {
      expect(e, isNull);
      return null;
    });
    expect(await store.entry('nope'), isNull);
  });

  test('keepBundle replaces the kept bytes', () async {
    final store = LineageStore(dir);
    await store.keepBundle('a1', Uint8List.fromList([1]));
    await store.keepBundle('a1', Uint8List.fromList([2, 2]));
    expect(await store.keptBundle('a1'), [2, 2]);
    expect(store.bundleFile('a1').path, '${dir.path}/bundles/a1.clax');
  });

  group('§5.2 export versions', () {
    const state = ReleaseState(
      contentSha256: _h1,
      title: 'Plant log',
      capabilities: {
        'db': {'write': true},
      },
    );

    test('a first export is version 1', () {
      expect(exportVersion(null, state), 1);
    });

    test('an unchanged release state keeps the version', () {
      expect(exportVersion(_author(version: 3), state), 3);
    });

    test('a changed hash, title or capability is the next version', () {
      final e = _author(version: 3);
      expect(
        exportVersion(
          e,
          const ReleaseState(
            contentSha256: _h2,
            title: 'Plant log',
            capabilities: {
              'db': {'write': true},
            },
          ),
        ),
        4,
      );
      expect(
        exportVersion(
          e,
          const ReleaseState(
            contentSha256: _h1,
            title: 'Plants',
            capabilities: {
              'db': {'write': true},
            },
          ),
        ),
        4,
      );
      expect(
        exportVersion(
          e,
          const ReleaseState(
            contentSha256: _h1,
            title: 'Plant log',
            capabilities: {},
          ),
        ),
        4,
      );
    });

    test('a release records its version, hash, title and capabilities', () {
      final e = _author(version: 1).withRelease(
        const ReleaseState(
          contentSha256: _h2,
          title: 'Plants',
          capabilities: {},
        ),
        version: 2,
      );
      expect(e.version, 2);
      expect(e.released, [_h1, _h2]);
      expect(e.title, 'Plants');
      expect(e.capabilities, isEmpty);
      // Releasing an earlier hash again moves it last, without a duplicate.
      final back = e.withRelease(state, version: 3);
      expect(back.released, [_h2, _h1]);
      expect(exportVersion(back, state), 3);
    });
  });

  group('§5.4 modified', () {
    final installed = _author(
      version: 2,
      released: const [_h1, _h2],
    ).copyWith(role: LineageRole.installed, title: 'Plant log');

    test('the latest release is unmodified', () {
      expect(
        installed.isModified(
          contentSha256: _h2,
          capabilities: {
            'db': {'write': true},
          },
        ),
        isFalse,
      );
    });

    test('a restore of an earlier release is unmodified', () {
      expect(
        installed.isModified(
          contentSha256: _h1,
          capabilities: {
            'db': {'write': true},
          },
        ),
        isFalse,
      );
    });

    test('another hash or other capabilities are modified', () {
      expect(
        installed.isModified(
          contentSha256: 'sha256:${'3' * 64}',
          capabilities: {
            'db': {'write': true},
          },
        ),
        isTrue,
      );
      expect(
        installed.isModified(
          contentSha256: _h2,
          capabilities: {
            'db': {'write': true},
            'notify': {},
          },
        ),
        isTrue,
      );
    });
  });

  test('lineage IDs are l_ and 26 lower-case base32 characters', () {
    final random = Random(7);
    final a = newLineageId(random);
    final b = newLineageId(random);
    expect(a, matches(RegExp(r'^l_[a-z2-7]{26}$')));
    expect(a, isNot(b));
  });
}
