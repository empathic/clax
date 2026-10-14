import 'dart:async';
import 'dart:typed_data';

import 'package:clax_app/agent/inspector.dart';
import 'package:clax_app/ui/app_surface.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/inspector_surface.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../agent/tools/support.dart' show StepClock;
import '../support/fake_surface.dart';

const _draft = 'd0000000000a';
final _base = Uri.parse('http://127.0.0.1:7999');
Uri _url([String aid = _draft]) =>
    Uri.parse('http://127.0.0.1:7999/a/$aid?app=1&inspect=1');

InspectionMsg _shot({List<({String kind, String text})> errors = const []}) =>
    InspectionMsg(
      png: Uint8List.fromList([1, 2, 3]),
      errors: errors,
      failed: const ['http://127.0.0.1:7999/missing.png'],
      text: 'Runs\n[button] Add',
      title: 'Runs',
      height: 900,
    );

Ready _ready([String aid = _draft]) =>
    Ready(artifact: aid, version: 1, latest: 1, file: 'index.html');

/// Lets the inspection's awaits run.
Future<void> _settle() async {
  for (var i = 0; i < 20; i++) {
    await Future<void>.delayed(Duration.zero);
  }
}

void main() {
  late StepClock clock;
  late List<FakeAppSurface> surfaces;
  late List<Uri> bases;
  late WebViewInspector inspector;

  setUp(() {
    clock = StepClock();
    surfaces = [];
    bases = [];
    inspector = WebViewInspector(
      surface: (base) {
        bases.add(base);
        final s = FakeAppSurface();
        surfaces.add(s);
        return s;
      },
      clock: clock,
    );
  });

  test(
    'an inspection shows the surface, loads the URL, waits for ready, '
    'asks the shell and returns its answer, then hides the surface',
    () async {
      final f = inspector.inspect(
        _url(),
        settle: const Duration(milliseconds: 1500),
        maxBytes: 4096,
      );
      await _settle();
      final s = surfaces.single;
      expect(bases, [_base]);
      expect(inspector.surface.value, same(s));
      expect(s.visibility, [true]);
      expect(s.loads, [_url()]);
      expect(s.hostCalls, isEmpty, reason: 'nothing is asked before ready');

      s.push(_ready());
      await _settle();
      expect(s.js, [const Inspect(settleMs: 1500, maxBytes: 4096).toJs()]);
      clock.advance(const Duration(milliseconds: 420));
      s.push(_shot(errors: const [(kind: 'error', text: 'x is not defined')]));
      final i = await f;
      expect(i.png, [1, 2, 3]);
      expect(i.errors.single.text, 'x is not defined');
      expect(i.uncaught, hasLength(1));
      expect(i.failed, ['http://127.0.0.1:7999/missing.png']);
      expect(i.text, 'Runs\n[button] Add');
      expect(i.title, 'Runs');
      expect(i.height, 900);
      expect(i.settledMs, 420);
      expect(s.visibility, [true, false]);
    },
  );

  test('a test run loads the URL, asks runTests after ready and returns '
      'its results; an inspection answer is not taken', () async {
    final f = inspector.runTests(
      _url(),
      files: const ['tests/a.test.js'],
      perTest: const Duration(seconds: 2),
      total: const Duration(seconds: 20),
    );
    await _settle();
    final s = surfaces.single;
    expect(s.loads, [_url()]);
    s.push(_ready());
    await _settle();
    expect(s.js, [
      const RunTests(['tests/a.test.js'], testMs: 2000, totalMs: 20000).toJs(),
    ]);
    s.push(_shot());
    s.push(
      const TestsMsg(
        passed: 1,
        failed: [(name: 'adds', error: 'Expected: 1 to be 2')],
        skipped: 2,
      ),
    );
    final r = await f;
    expect(r.toJson(), {
      'passed': 1,
      'failed': [
        {'name': 'adds', 'error': 'Expected: 1 to be 2'},
      ],
      'skipped': 2,
    });
    expect(s.visibility, [true, false]);
  });

  test("a test run's error answer, and no answer within the run's time, "
      'are InspectorErrors', () async {
    var f = inspector.runTests(_url(), files: const ['tests/a.test.js']);
    await _settle();
    surfaces.single.push(_ready());
    await _settle();
    surfaces.single.push(const TestsMsg.failed('the page has not loaded'));
    await expectLater(
      f,
      throwsA(
        isA<InspectorError>().having(
          (e) => e.message,
          'message',
          'the page has not loaded',
        ),
      ),
    );
    f = inspector.runTests(
      _url(),
      files: const ['tests/a.test.js'],
      total: const Duration(seconds: 20),
    );
    await _settle();
    surfaces.single.push(_ready());
    await _settle();
    clock.advance(const Duration(seconds: 30));
    await expectLater(
      f,
      throwsA(
        isA<InspectorError>().having(
          (e) => e.message,
          'message',
          contains('did not answer within 30 seconds'),
        ),
      ),
    );
  });

  final migrateView = Uri.parse(
    'http://127.0.0.1:7999/a/$_draft?app=1&inspect=1&migrate=1',
  );

  test('a migration run loads the migration view, asks runMigration after '
      'ready, and returns ok, or the error the page gave, once the view is '
      'loaded again', () async {
    var done = false;
    var f = inspector.runMigration(
      _url(),
      from: 2,
      to: 3,
      total: const Duration(seconds: 20),
    )..whenComplete(() => done = true);
    await _settle();
    final s = surfaces.single;
    expect(s.loads, [migrateView]);
    s.push(_ready());
    await _settle();
    expect(s.js, [const RunMigration(from: 2, to: 3, totalMs: 20000).toJs()]);
    // Another call's answer is not taken.
    s.push(const TestsMsg(passed: 1, failed: [], skipped: 0));
    s.push(const MigrationMsg.ok());
    await _settle();
    // The document the migration ran in is replaced before the run returns.
    expect(s.loads, [migrateView, migrateView]);
    expect(done, isFalse);
    s.push(_ready());
    final r = await f;
    expect((r.ok, r.error), (true, null));
    expect(s.visibility.last, isFalse);

    f = inspector.runMigration(_url(), from: 1, to: 2);
    await _settle();
    s.push(_ready());
    await _settle();
    s.push(const MigrationMsg.failed('the migration failed: boom'));
    await _settle();
    expect(s.loads, hasLength(4));
    s.push(_ready());
    final no = await f;
    expect((no.ok, no.error), (false, 'the migration failed: boom'));
  });

  test('a migration run that never answers, or is cancelled, is an '
      'InspectorError only once its document is replaced', () async {
    var f = inspector.runMigration(
      _url(),
      from: 1,
      to: 2,
      total: const Duration(seconds: 20),
    );
    await _settle();
    final s = surfaces.single;
    s.push(_ready());
    await _settle();
    clock.advance(const Duration(seconds: 30));
    await _settle();
    expect(s.loads, [migrateView, migrateView]);
    s.push(_ready());
    await expectLater(
      f,
      throwsA(
        isA<InspectorError>().having(
          (e) => e.message,
          'message',
          contains('did not answer within 30 seconds'),
        ),
      ),
    );
    final stop = Completer<void>();
    f = inspector.runMigration(_url(), from: 1, to: 2, cancelled: stop.future);
    await _settle();
    s.push(_ready());
    await _settle();
    stop.complete();
    await _settle();
    expect(s.loads, hasLength(4));
    // No ready for the replacement: it waits its limit, then returns.
    clock.advance(const Duration(seconds: 10));
    await expectLater(
      f,
      throwsA(
        isA<InspectorError>().having((e) => e.message, 'message', 'cancelled'),
      ),
    );
  });

  test('every inspection loads a new document in the same surface', () async {
    for (var n = 0; n < 2; n++) {
      final f = inspector.inspect(_url());
      await _settle();
      surfaces.single.push(_ready());
      await _settle();
      surfaces.single.push(_shot());
      await f;
    }
    expect(surfaces, hasLength(1), reason: 'one WebView is reused');
    expect(surfaces.single.loads, [_url(), _url()]);
    expect(surfaces.single.visibility, [true, false, true, false]);
  });

  test(
    'a new daemon address makes a new surface and disposes the old one',
    () async {
      var f = inspector.inspect(_url());
      await _settle();
      surfaces.single.push(_ready());
      await _settle();
      surfaces.single.push(_shot());
      await f;
      final moved = Uri.parse(
        'http://127.0.0.1:8123/a/$_draft?app=1&inspect=1',
      );
      f = inspector.inspect(moved);
      await _settle();
      expect(bases, [_base, Uri.parse('http://127.0.0.1:8123')]);
      expect(surfaces.first.disposed, isTrue);
      surfaces.last.push(_ready());
      await _settle();
      surfaces.last.push(_shot());
      await f;
      expect(inspector.surface.value, same(surfaces.last));
    },
  );

  test("another artifact's ready, and an inspection before asking, are "
      'not taken', () async {
    final f = inspector.inspect(_url());
    await _settle();
    final s = surfaces.single;
    s.push(_shot());
    s.push(_ready('a0000000000b'));
    await _settle();
    expect(s.hostCalls, isEmpty);
    s.push(_ready());
    await _settle();
    expect(s.hostCalls, hasLength(1));
    s.push(_shot());
    await f;
  });

  test('no ready within the limit is an InspectorError', () async {
    final f = inspector.inspect(_url());
    await _settle();
    clock.advance(const Duration(seconds: 10));
    await expectLater(
      f,
      throwsA(
        isA<InspectorError>().having(
          (e) => e.message,
          'message',
          contains('did not load within 10 seconds'),
        ),
      ),
    );
    expect(surfaces.single.visibility.last, isFalse);
  });

  test(
    'no answer within the settle time and the limit is an InspectorError',
    () async {
      final f = inspector.inspect(_url(), settle: const Duration(seconds: 3));
      await _settle();
      surfaces.single.push(_ready());
      await _settle();
      clock.advance(const Duration(seconds: 12));
      await _settle();
      clock.advance(const Duration(seconds: 1));
      await expectLater(
        f,
        throwsA(
          isA<InspectorError>().having(
            (e) => e.message,
            'message',
            contains('did not answer within 13 seconds'),
          ),
        ),
      );
    },
  );

  test("the shell's error answer, and a load the surface gave up on, are "
      'InspectorErrors', () async {
    var f = inspector.inspect(_url());
    await _settle();
    surfaces.single.push(_ready());
    await _settle();
    surfaces.single.push(InspectionMsg.failed('the page sent no screenshot'));
    await expectLater(
      f,
      throwsA(
        isA<InspectorError>().having(
          (e) => e.message,
          'message',
          'the page sent no screenshot',
        ),
      ),
    );

    f = inspector.inspect(_url());
    await _settle();
    surfaces.single.fault(
      const SurfaceFault('net::ERR_CONNECTION_REFUSED', reloading: true),
    );
    await _settle();
    surfaces.single.fault(
      const SurfaceFault('net::ERR_CONNECTION_REFUSED', reloading: false),
    );
    await expectLater(
      f,
      throwsA(
        isA<InspectorError>().having(
          (e) => e.message,
          'message',
          'net::ERR_CONNECTION_REFUSED',
        ),
      ),
    );
  });

  test('a cancel ends the inspection at once', () async {
    final cancel = Completer<void>();
    final f = inspector.inspect(_url(), cancelled: cancel.future);
    await _settle();
    surfaces.single.push(_ready());
    await _settle();
    cancel.complete();
    await expectLater(
      f,
      throwsA(isA<InspectorError>().having((e) => e.message, 'm', 'cancelled')),
    );
  });

  test('inspections run one at a time, in order; one cancelled while it '
      'waits never loads', () async {
    final first = inspector.inspect(_url());
    final cancel = Completer<void>();
    final second = inspector.inspect(_url(), cancelled: cancel.future);
    final third = inspector.inspect(_url());
    await _settle();
    final s = surfaces.single;
    expect(s.loads, hasLength(1), reason: 'the second waits for the first');
    cancel.complete();
    await expectLater(
      second,
      throwsA(isA<InspectorError>().having((e) => e.message, 'm', 'cancelled')),
    );
    await _settle();
    expect(s.loads, hasLength(1), reason: 'the third still waits');

    s.push(_ready());
    await _settle();
    s.push(_shot());
    await first;
    await _settle();
    expect(s.loads, hasLength(2), reason: 'the third goes after the first');
    s.push(_ready());
    await _settle();
    s.push(_shot());
    await third;
    expect(s.loads, hasLength(2));
  });

  test('warm makes the surface and loads the inspect view once, hidden; '
      'the first inspection reuses it', () async {
    await inspector.warm(_base);
    await inspector.warm(_base);
    final s = surfaces.single;
    expect(inspector.surface.value, same(s));
    expect(s.loads, [
      Uri.parse('http://127.0.0.1:7999/a/000000000000?app=1&inspect=1'),
    ]);
    expect(s.visibility, [false]);
    final f = inspector.inspect(_url());
    await _settle();
    expect(surfaces, hasLength(1));
    s.push(_ready());
    await _settle();
    s.push(_shot());
    await f;
    expect(s.loads.last, _url());
  });

  test('warm waits for an inspection in progress; a new address warms a '
      'new surface', () async {
    final f = inspector.inspect(_url());
    final w = inspector.warm(Uri.parse('http://127.0.0.1:8123'));
    await _settle();
    expect(surfaces, hasLength(1));
    surfaces.single.push(_ready());
    await _settle();
    surfaces.single.push(_shot());
    await f;
    await w;
    expect(surfaces, hasLength(2));
    expect(surfaces.first.disposed, isTrue);
    expect(surfaces.last.loads.single.port, 8123);
  });

  testWidgets('the host keeps the surface laid out at its size under an '
      'opaque cover and the app, never Offstage', (tester) async {
    await tester.pumpWidget(
      Directionality(
        textDirection: TextDirection.ltr,
        child: InspectorHost(
          inspector: inspector,
          size: const Size(390, 600),
          cover: const Color(0x00FFFFFF),
          child: const Center(child: Text('the app')),
        ),
      ),
    );
    expect(find.byKey(const ValueKey('fake-surface')), findsNothing);
    unawaited(inspector.inspect(_url()).then((_) {}, onError: (Object _) {}));
    await tester.runAsync(_settle);
    await tester.pump();
    final page = find.byKey(const ValueKey('fake-surface'));
    expect(page, findsOneWidget);
    expect(tester.getSize(page), const Size(390, 600));
    expect(tester.getTopLeft(page), Offset.zero);
    expect(find.byType(Offstage), findsNothing);
    final cover = tester.widget<ColoredBox>(
      find.byKey(const ValueKey('inspector-cover')),
    );
    expect(cover.color.a, 1, reason: 'the cover is opaque');
    expect(find.text('the app'), findsOneWidget);
    // The app is painted above the cover, which is above the page.
    final stack = tester.widget<Stack>(
      find.ancestor(of: page, matching: find.byType(Stack)).first,
    );
    expect(stack.children.length, 3);
    clock.advance(const Duration(seconds: 10));
    await tester.pump();
  });

  test('the inspection size is the screen on a phone, at most a phone on a '
      'desktop, and a phone when the screen is not known', () {
    expect(
      inspectorSize(const Size(412, 915), desktop: false),
      const Size(412, 915),
    );
    expect(
      inspectorSize(const Size(1440, 900), desktop: true),
      const Size(390, 844),
    );
    expect(
      inspectorSize(const Size(360, 700), desktop: true),
      const Size(360, 700),
    );
    expect(inspectorSize(Size.zero, desktop: false), const Size(390, 844));
  });
}
