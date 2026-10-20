import 'dart:convert';
import 'dart:io';

import 'package:clax_app/core/core_client.dart';
import 'package:clax_app/core/core_host.dart';
import 'package:clax_app/core/frame.dart';
import 'package:clax_app/core/origins.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_core.dart';
import '../support/paths.dart';

void main() {
  late Directory support;
  late FakeCoreLib lib;
  late String? key;
  late int cookieClears;
  late List<List<String>> excluded;
  late String home;

  setUp(() {
    support = scratchRoot().createTempSync('core-host-');
    home = '${support.path}/clax-home';
    lib = FakeCoreLib();
    key = null;
    cookieClears = 0;
    excluded = [];
  });
  tearDown(() => support.deleteSync(recursive: true));

  CoreHost host({bool backup = false}) => CoreHost(
    home: home,
    supportDir: support.path,
    origins: AppOrigins.forPlatform(android: true),
    apiKey: () async => key,
    lib: lib,
    pid: 4242,
    clearCookies: () async => cookieClears++,
    excludeFromBackup: backup ? (paths) async => excluded.add(paths) : null,
  );

  /// The key puts the core was sent, as their JSON bodies.
  List<Object?> keyPuts() => [
    for (final c in lib.calls)
      if (c.request.path == '/api/sample/key')
        [c.request.method, jsonDecode(utf8.decode(c.request.body!))],
  ];

  Map<String, Object?>? readiness() {
    final f = File('${support.path}/${CoreHost.readinessFile}');
    return f.existsSync()
        ? jsonDecode(f.readAsStringSync()) as Map<String, Object?>
        : null;
  }

  group('open', () {
    test('deletes daemon-port, writes config.toml and opens once', () async {
      File('${support.path}/daemon-port').writeAsStringSync('7555');
      final h = host();
      expect(h.state.value, isA<CoreOpening>());
      h.open();
      h.open();
      expect(lib.opens, hasLength(1));
      expect(lib.opens.single.home, home);
      expect(File('${support.path}/daemon-port').existsSync(), isFalse);
      expect(
        File('$home/config.toml').readAsStringSync(),
        CoreHost.sampleConfig,
      );
      expect(CoreHost.sampleConfig, contains('[sample.models]'));
      expect(h.handle, lib.openAnswer);
    });

    test('puts the key once the core is ready, and not before', () async {
      key = 'sk-test';
      final h = host()..open();
      lib.postState({'type': 'opening', 'phase': 'migrating', 'done': 0, 'total': 1});
      await pumpEventQueue();
      expect(keyPuts(), isEmpty);
      lib.postState({'type': 'ready'});
      await pumpEventQueue();
      expect(h.state.value, const CoreReady());
      expect(keyPuts(), [
        ['PUT', {'key': 'sk-test'}],
      ]);
      expect(lib.calls.single.request.as, CallAs.owner);
    });

    test('puts no key at open when there is none', () async {
      host().open();
      lib.postState({'type': 'ready'});
      await pumpEventQueue();
      expect(keyPuts(), isEmpty);
    });

    test('a failed open ends in CoreFailed with the backup', () async {
      final h = host()..open();
      lib.postState({
        'type': 'failed',
        'code': 'migration_failed',
        'message': 'migration 44 failed',
        'backup': '/b/clax.db.bak',
      });
      await pumpEventQueue();
      expect(
        h.state.value,
        const CoreFailed('migration_failed', 'migration 44 failed', '/b/clax.db.bak'),
      );
      expect(readiness(), {
        'pid': 4242,
        'state': 'failed',
        'code': 'migration_failed',
      });
    });

    test('never goes back to opening after ready', () async {
      final h = host()..open();
      final seen = <CoreOpen>[];
      h.state.addListener(() => seen.add(h.state.value));
      lib.postState({'type': 'ready'});
      lib.postState({'type': 'opening', 'phase': 'migrating'});
      await pumpEventQueue();
      expect(seen, [const CoreReady()]);
      expect(h.state.value, const CoreReady());
    });

    test('says it is ready in the support directory, with the PID', () async {
      File('${support.path}/${CoreHost.readinessFile}')
          .writeAsStringSync('{"pid": 1, "state": "ready"}');
      host().open();
      expect(readiness(), isNull, reason: 'an earlier launch\'s is removed');
      lib.postState({'type': 'opening', 'phase': 'backing up'});
      await pumpEventQueue();
      expect(readiness(), isNull);
      lib.postState({'type': 'ready'});
      await pumpEventQueue();
      expect(readiness(), {'pid': 4242, 'state': 'ready'});
    });

    test('clears the WebViews\' cookies once per install', () async {
      host().open();
      await pumpEventQueue();
      expect(cookieClears, 1);
      expect(File('${support.path}/.cookies-cleared').existsSync(), isTrue);
      host().open();
      await pumpEventQueue();
      expect(cookieClears, 1);
    });

    test('marks the run files as excluded from backup once ready', () async {
      host(backup: true).open();
      await pumpEventQueue();
      expect(excluded, isEmpty);
      lib.postState({'type': 'ready'});
      await pumpEventQueue();
      expect(excluded, [
        [for (final n in CoreHost.runFiles) '$home/$n'],
      ]);
    });
  });

  group('keyChanged', () {
    test('puts a new key, and clears it when there is none', () async {
      key = 'sk-one';
      lib.answerWith = 204;
      final h = host()..open();
      lib.postState({'type': 'ready'});
      await pumpEventQueue();
      key = null;
      await h.keyChanged();
      key = 'sk-two';
      await h.keyChanged();
      expect(keyPuts(), [
        ['PUT', {'key': 'sk-one'}],
        ['PUT', {'key': null}],
        ['PUT', {'key': 'sk-two'}],
      ]);
    });

    test('before the core is ready, waits for it', () async {
      lib.answerWith = 204;
      final h = host()..open();
      key = 'sk-early';
      final changed = h.keyChanged();
      await pumpEventQueue();
      expect(keyPuts(), isEmpty);
      lib.postState({'type': 'ready'});
      await changed;
      expect(keyPuts().last, ['PUT', {'key': 'sk-early'}]);
    });
  });

  test('trim asks the core to trim its memory', () async {
    final h = host()..open();
    lib.postState({'type': 'ready'});
    await pumpEventQueue();
    final trimmed = h.trim();
    await pumpEventQueue();
    final c = lib.calls.singleWhere((c) => c.request.path == '/api/core/trim');
    expect(c.request.method, 'POST');
    lib.answer(c, 204);
    await trimmed;
  });
}
