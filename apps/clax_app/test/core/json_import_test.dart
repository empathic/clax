import 'dart:convert';
import 'dart:io';

import 'package:clax_app/agent/asked.dart';
import 'package:clax_app/agent/ledger.dart';
import 'package:clax_app/agent/plan.dart';
import 'package:clax_app/agent/stops.dart';
import 'package:clax_app/agent/unfinished.dart';
import 'package:clax_app/clax/api.dart' show ClaxApiError;
import 'package:clax_app/core/core_transport.dart';
import 'package:clax_app/core/frame.dart';
import 'package:clax_app/core/json_import.dart';
import 'package:clax_app/platform/pending_make.dart';
import 'package:clax_app/ui/read_consent_dialog.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_clock.dart';
import '../support/fake_core.dart';
import '../support/paths.dart';

/// The fixtures' artifacts (test/fixtures/json-state).
const _reading = 'k3m9q2v7x1ab';
const _habits = 'p4n8r6t2w5cd';

/// The fixture [name]'s text.
String fixture(String name) =>
    File(appPath('test/fixtures/json-state/$name')).readAsStringSync();

/// The core's import route on a [FakeCore], as
/// crates/clax-server/src/routes/client_imports.rs answers it: the first
/// import of the key with the owner's token answers its summary, every
/// later one the same with `already: true`.
class _ImportRoute {
  _ImportRoute(this.core) {
    core.on('POST', '/api/client-imports/{key}', (req, p) {
      requests.add(req);
      if (p['key'] != jsonImportKey) {
        return FakeCore.error(400, 'invalid_args', 'no import is named it');
      }
      if (req.headers.where((h) => h.$1 == 'x-clax-jobs-owner').firstOrNull?.$2 !=
          owner) {
        return FakeCore.error(409, 'not_owner', 'another worker holds it');
      }
      final body = FakeCore.bodyOf(req)! as Map<String, Object?>;
      bodies.add(body);
      final first = summary == null;
      summary ??= {
        'jobs': (body['files']! as Map).length,
        'consents': 0,
        'skipped': <String>[],
        'left_out': ['stopped.json: the thread 01JA is gone'],
        'done_at': '2026-10-12T09:00:00Z',
      };
      return FakeCore.json({
        'import': {...summary!, 'already': !first},
      });
    });
  }

  final FakeCore core;
  static const owner = 'token-1';
  final List<CoreRequest> requests = [];
  final List<Map<String, Object?>> bodies = [];
  Map<String, Object?>? summary;
}

void main() {
  late Directory support;
  late FakeCore core;
  late _ImportRoute route;
  late FakeClock clock;

  setUp(() {
    support = scratchRoot().createTempSync('json-import-');
    addTearDown(() => support.deleteSync(recursive: true));
    core = FakeCore();
    route = _ImportRoute(core);
    clock = FakeClock();
  });

  /// Puts the fixtures [names] (all seven by default) in the support
  /// directory.
  void seed([List<String> names = jsonStateFiles]) {
    for (final n in names) {
      File('${support.path}/$n').writeAsStringSync(fixture(n));
    }
  }

  Future<ImportSummary?> run({void Function(File f)? delete}) =>
      importJsonState(
        supportDir: support.path,
        core: CoreTransport(core),
        ownerToken: 'token-1',
        clock: clock,
        delete: delete ?? (f) => f.deleteSync(),
      );

  List<String> left() => [
    for (final n in jsonStateFiles)
      if (File('${support.path}/$n').existsSync()) n,
  ];

  List<Directory> copies() {
    final d = Directory('${support.path}/$importedJsonDir');
    if (!d.existsSync()) return [];
    return d.listSync().whereType<Directory>().toList()
      ..sort((a, b) => a.path.compareTo(b.path));
  }

  group('the fixtures are what the app writes', () {
    Object? decoded(String name) => jsonDecode(fixture(name));
    Map<String, Object?> obj(Object? v) => (v! as Map).cast<String, Object?>();

    test('pending-make.json, by PendingMake', () async {
      final f = File('${support.path}/pending-make.json');
      final p = PendingMake(f);
      await p.save('a tracker for my runs');
      await p.save('a habit log with streaks');
      await p.waiting('a habit log with streaks', true);
      expect(f.readAsStringSync(), fixture('pending-make.json'));
    });

    test('stopped.json, by FileStoppedThreads', () async {
      final f = File('${support.path}/stopped.json');
      await FileStoppedThreads(
        f,
      ).put('01JA8Z6T1B2C3D4E5F6G7H8J9K', '01JA8Z6C1B2C3D4E5F6G7H8J9K');
      expect(f.readAsStringSync(), fixture('stopped.json'));
    });

    test('agent-read-consent.json, by FileConsentMemory', () async {
      final f = File('${support.path}/agent-read-consent.json');
      await FileConsentMemory(f).allow(_reading, _habits);
      expect(f.readAsStringSync(), fixture('agent-read-consent.json'));
    });

    test('unfinished.json, by FileUnfinishedChanges', () async {
      final f = File('${support.path}/unfinished.json');
      final entry = obj(obj(decoded('unfinished.json'))['threads']).values;
      await FileUnfinishedChanges(
        f,
      ).put(UnfinishedChange.fromJson(obj(entry.single)));
      expect(f.readAsStringSync(), fixture('unfinished.json'));
    });

    test('change-ledger.json, by ChangeRecord', () {
      final records = obj(decoded('change-ledger.json'))['records']! as List;
      expect({
        'records': [
          for (final r in records) ChangeRecord.fromJson(obj(r)).toJson(),
        ],
      }, decoded('change-ledger.json'));
    });

    test('plans.json, by Plan', () {
      final plans = obj(obj(decoded('plans.json'))['plans']);
      expect({
        'plans': {
          for (final e in plans.entries)
            e.key: Plan.fromJson(obj(e.value)).toJson(),
        },
      }, decoded('plans.json'));
    });

    test('asked.json, by AskedEntry', () {
      final j = obj(decoded('asked.json'));
      expect({
        'version': 1,
        'entries': [
          for (final e in j['entries']! as List) AskedEntry.fromJson(e).toJson(),
        ],
      }, j);
    });
  });

  test('sends every file as the app wrote it, under the owner\'s token, '
      'then copies them and deletes them', () async {
    seed();
    final s = await run();

    expect(route.requests, hasLength(1));
    final r = route.requests.single;
    expect((r.method, r.path), ('POST', '/api/client-imports/app-json-v1'));
    expect(r.headers, contains(('x-clax-jobs-owner', 'token-1')));
    expect(route.bodies.single, {
      'client': 'app',
      'files': {for (final n in jsonStateFiles) n: fixture(n)},
    });

    expect(s, isNotNull);
    expect(s!.already, isFalse);
    expect(s.jobs, 7);
    expect(s.leftOut, ['stopped.json: the thread 01JA is gone']);
    expect(s.doneAt, '2026-10-12T09:00:00Z');
    expect(s.copiedTo, '${support.path}/imported-json/2026-10-12T09-00-00.000Z');

    expect(left(), isEmpty);
    final copied = copies().single;
    expect(copied.path, s.copiedTo);
    for (final n in jsonStateFiles) {
      expect(File('${copied.path}/$n').readAsStringSync(), fixture(n));
    }
  });

  test('only the files there are sent', () async {
    seed(['plans.json', 'change-ledger.json']);
    await run();
    expect((route.bodies.single['files']! as Map).keys, [
      'plans.json',
      'change-ledger.json',
    ]);
    expect(left(), isEmpty);
  });

  test('no files make no call', () async {
    expect(await run(), isNull);
    expect(core.requests, isEmpty);
    expect(copies(), isEmpty);
  });

  test('a run after the import sends what is there, is answered already, '
      'and deletes it too', () async {
    seed();
    await run();
    expect(await run(), isNull, reason: 'nothing left: no call');
    expect(route.requests, hasLength(1));

    seed(['stopped.json']);
    await clock.advance(const Duration(minutes: 1));
    final again = await run();
    expect(again!.already, isTrue);
    expect(again.jobs, 7, reason: "the first import's summary");
    expect(left(), isEmpty);
    expect(copies(), hasLength(2));
    expect(
      File('${copies().last.path}/stopped.json').readAsStringSync(),
      fixture('stopped.json'),
    );
  });

  test('cut off between the core\'s commit and the deletes, the next run '
      'is harmless', () async {
    seed();
    var deletes = 0;
    await expectLater(
      run(
        delete: (f) {
          // The process ends after the first delete.
          if (deletes++ > 0) throw FileSystemException('killed', f.path);
          f.deleteSync();
        },
      ),
      throwsA(isA<FileSystemException>()),
    );
    expect(left(), hasLength(6), reason: 'one deleted before the crash');
    expect(copies(), hasLength(1), reason: 'copied before any delete');

    final again = await run();
    expect(again!.already, isTrue);
    expect(route.requests, hasLength(2));
    expect(route.bodies.last['files'], {
      for (final n in jsonStateFiles.skip(1)) n: fixture(n),
    });
    expect(left(), isEmpty);
    // The same second: the second copy lands in the first's directory.
    expect(copies(), hasLength(1));
    for (final n in jsonStateFiles) {
      expect(File('${copies().single.path}/$n').readAsStringSync(), fixture(n));
    }
  });

  test('a refused import keeps the files and copies nothing', () async {
    seed(['asked.json']);
    await expectLater(
      importJsonState(
        supportDir: support.path,
        core: CoreTransport(core),
        ownerToken: 'not-the-owner',
        clock: clock,
      ),
      throwsA(
        isA<ClaxApiError>()
            .having((e) => e.status, 'status', 409)
            .having((e) => e.code, 'code', 'not_owner'),
      ),
    );
    expect(left(), ['asked.json']);
    expect(copies(), isEmpty);
  });

  test('a file that is not UTF-8 is still sent, for the core to skip', () async {
    File('${support.path}/plans.json').writeAsBytesSync([0x7b, 0xff, 0xfe]);
    await run();
    expect(route.bodies.single['files'], {'plans.json': '{\u{fffd}\u{fffd}'});
    expect(
      File('${copies().single.path}/plans.json').readAsBytesSync(),
      [0x7b, 0xff, 0xfe],
      reason: 'the copy keeps the bytes as they were',
    );
  });
}
