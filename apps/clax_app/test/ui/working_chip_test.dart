import 'dart:io';
import 'dart:ui' as ui;

import 'package:clax_app/agent/failure.dart';
import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/daemon/clock.dart';
import 'package:clax_app/ui/change_tracker.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/working_chip.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

/// A clock the test moves by hand.
class _Clock implements Clock {
  DateTime t = DateTime.utc(2026, 10, 12, 9);
  @override
  DateTime now() => t;
  @override
  Future<void> delay(Duration d) async {}
}

/// Bytes standing for a screenshot; images do not decode under the test's
/// fake async, so the thumbnail's frame is what is checked.
final _png = Uint8List.fromList(const [0x89, 0x50, 0x4e, 0x47]);

class _Rig {
  _Rig() {
    tracker = ChangeTracker(
      timings: const NoTimings(),
      artifact: 'a1',
      now: clock.now,
    )..shown(3, pinned: false);
  }

  final clock = _Clock();
  late final ChangeTracker tracker;
  final log = <String>[];

  /// The chip as the app screen places it, over a page.
  Widget build({
    Brightness brightness = Brightness.light,
    double textScale = 1,
    bool retry = true,
    Key? boundary,
  }) => MaterialApp(
    debugShowCheckedModeBanner: false,
    theme: claxTheme(brightness, defaultTargetPlatform),
    home: MediaQuery.withClampedTextScaling(
      minScaleFactor: textScale,
      maxScaleFactor: textScale,
      child: StatefulBuilder(
        builder: (context, setState) => RepaintBoundary(
          key: boundary,
          child: Scaffold(
            body: Stack(
              fit: StackFit.expand,
              children: [
                const _Page(),
                SafeArea(
                  child: Stack(
                    children: [
                      Positioned(
                        top: 8,
                        left: 24,
                        right: 24,
                        child: Center(
                          child: WorkingChip(
                            changes: tracker,
                            clock: clock,
                            onDismiss: () {
                              log.add('dismiss');
                              setState(tracker.dismiss);
                            },
                            onReplied: () => log.add('replied'),
                            onResend: () => log.add('resend'),
                            onRetry: retry ? () => log.add('retry') : null,
                            onOpen: () => log.add('open'),
                          ),
                        ),
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    ),
  );

  void narrate(List<String> steps) {
    for (final s in steps) {
      tracker.workerProgress(const ['t1'], Narration(s));
    }
  }
}

/// A page under the chip, like a small tracker app.
class _Page extends StatelessWidget {
  const _Page();

  @override
  Widget build(BuildContext context) => ColoredBox(
    color: const Color(0xfff7f4ee),
    child: Padding(
      padding: const EdgeInsets.fromLTRB(20, 96, 20, 20),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text(
            'Runs',
            style: TextStyle(
              fontSize: 32,
              fontWeight: FontWeight.w500,
              color: Color(0xff1d1b16),
            ),
          ),
          const SizedBox(height: 12),
          for (final (d, km) in const [
            ('Mon', '5.2 km'),
            ('Wed', '8.0 km'),
            ('Sat', '12.4 km'),
          ])
            Container(
              margin: const EdgeInsets.only(bottom: 8),
              padding: const EdgeInsets.all(14),
              decoration: BoxDecoration(
                color: Colors.white,
                borderRadius: BorderRadius.circular(10),
                border: Border.all(color: const Color(0xffe2ddd2)),
              ),
              child: Row(
                children: [
                  Text(d, style: const TextStyle(color: Color(0xff1d1b16))),
                  const Spacer(),
                  Text(km, style: const TextStyle(color: Color(0xff1d1b16))),
                ],
              ),
            ),
        ],
      ),
    ),
  );
}

void main() {
  void phone(WidgetTester tester) {
    tester.view.devicePixelRatio = 3;
    tester.view.physicalSize = const Size(360, 780) * 3;
    tester.view.padding = const FakeViewPadding(top: 72);
    addTearDown(tester.view.reset);
  }

  testWidgets('before a step it says Working…; then the latest step', (
    tester,
  ) async {
    phone(tester);
    final rig = _Rig();
    rig.tracker
      ..sent('t1')
      ..workerWorking(const ['t1']);
    await tester.pumpWidget(rig.build());
    expect(find.text('Working…'), findsOneWidget);
    expect(find.byKey(const ValueKey('chip-elapsed')), findsNothing);

    rig.narrate(['Reading your runs', 'Adding the weekly total']);
    rig.clock.t = rig.clock.t.add(const Duration(seconds: 12));
    await tester.pump(const Duration(seconds: 1));
    expect(find.text('Adding the weekly total'), findsOneWidget);
    expect(find.text('Reading your runs'), findsNothing, reason: 'collapsed');
    expect(find.text('12s'), findsOneWidget);

    rig.clock.t = rig.clock.t.add(const Duration(seconds: 63));
    await tester.pump(const Duration(seconds: 1));
    expect(find.text('1m 15s'), findsOneWidget);
  });

  testWidgets('a tap opens the change in every running state', (tester) async {
    phone(tester);
    final rig = _Rig();
    rig.tracker
      ..sent('t1')
      ..workerWorking(const ['t1']);
    final states = <String, void Function()>{
      'Working…': () {},
      'Reading your runs': () => rig.narrate(['Reading your runs']),
      'Loading the new version': () => rig.tracker.workerLanded(4),
    };
    for (final MapEntry(key: label, value: set) in states.entries) {
      set();
      await tester.pumpWidget(rig.build());
      expect(find.text(label), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('chip-working')));
      await tester.pump();
    }
    expect(rig.log, ['open', 'open', 'open']);

    // Trying again: a Retry the worker has not started yet.
    final again = _Rig();
    again.tracker
      ..workerWorking(const ['t1'])
      ..workerFailed('the connection dropped')
      ..retryStarted()
      ..retried();
    await tester.pumpWidget(again.build());
    expect(find.text('Trying again…'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('chip-working')));
    expect(again.log, ['open']);
  });

  testWidgets('the live region is the step alone: elapsed time is silent', (
    tester,
  ) async {
    phone(tester);
    final handle = tester.ensureSemantics();
    final rig = _Rig();
    rig.tracker
      ..sent('t1')
      ..workerWorking(const ['t1']);
    rig.narrate(['Changing index.html']);
    await tester.pumpWidget(rig.build());

    List<String> live() {
      final out = <String>[];
      void visit(SemanticsNode n) {
        if (n.getSemanticsData().flagsCollection.isLiveRegion) {
          out.add(n.getSemanticsData().label);
        }
        n.visitChildren((c) {
          visit(c);
          return true;
        });
      }

      visit(
        tester
            .renderObject(find.byType(MaterialApp))
            .owner!
            .semanticsOwner!
            .rootSemanticsNode!,
      );
      return out;
    }

    final before = live();
    expect(before, ['Changing index.html']);
    for (var i = 0; i < 3; i++) {
      rig.clock.t = rig.clock.t.add(const Duration(seconds: 1));
      await tester.pump(const Duration(seconds: 1));
      expect(find.text('${i + 1}s'), findsOneWidget);
      expect(live(), before, reason: 'a tick is not announced');
    }
    rig.narrate(['Checking how it looks']);
    await tester.pumpWidget(rig.build());
    expect(live(), ['Checking how it looks'], reason: 'a new step is');

    expect(
      tester.getSemantics(
        find
            .descendant(
              of: find.byKey(const ValueKey('chip-working')),
              matching: find.byType(InkWell),
            )
            .first,
      ),
      matchesSemantics(
        onTapHint: 'Show the change',
        hasTapAction: true,
        isButton: true,
        isFocusable: true,
        hasFocusAction: true,
      ),
    );
    handle.dispose();
  });

  testWidgets('a failed change shows its reason and a Retry', (tester) async {
    phone(tester);
    final rig = _Rig();
    rig.tracker
      ..workerWorking(const ['t1'])
      ..workerFailed('the connection dropped');
    await tester.pumpWidget(rig.build());
    expect(find.byKey(const ValueKey('chip-failed')), findsOneWidget);
    expect(find.textContaining('the connection dropped'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('retry-change')));
    expect(rig.log, ['retry']);

    rig.tracker.retryStarted();
    await tester.pumpWidget(rig.build());
    final button = tester.widget<FilledButton>(
      find.byKey(const ValueKey('retry-change')),
    );
    expect(button.onPressed, isNull, reason: 'one Retry at a time');
    expect(find.text('Retrying…'), findsOneWidget);
  });

  testWidgets('a change stopped at a limit offers Keep going', (tester) async {
    phone(tester);
    final rig = _Rig();
    rig.tracker
      ..workerWorking(const ['t1'])
      ..workerFailed(limitFailureReason('10 turns', leftUsd: 6));
    await tester.pumpWidget(rig.build());
    expect(find.text('Keep going'), findsOneWidget);
    expect(find.textContaining('more than 10 steps'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('retry-change')));
    expect(rig.log, ['retry']);
  });

  testWidgets('a limit stop that spent the change’s limit offers no Retry', (
    tester,
  ) async {
    phone(tester);
    final rig = _Rig();
    rig.tracker
      ..workerWorking(const ['t1'])
      ..workerFailed(limitFailureReason('10 turns', leftUsd: 0));
    await tester.pumpWidget(rig.build());
    expect(find.textContaining('so it can’t keep going'), findsOneWidget);
    expect(find.byKey(const ValueKey('retry-change')), findsNothing);
    expect(find.text('Retry'), findsNothing);
  });

  testWidgets('without a retry hook a failed change offers no Retry', (
    tester,
  ) async {
    phone(tester);
    final rig = _Rig();
    rig.tracker
      ..workerWorking(const ['t1'])
      ..workerFailed('x');
    await tester.pumpWidget(rig.build(retry: false));
    expect(find.byKey(const ValueKey('retry-change')), findsNothing);
  });

  testWidgets('a refused Retry says why and offers none', (tester) async {
    phone(tester);
    final rig = _Rig();
    rig.tracker
      ..workerWorking(const ['t1'])
      ..workerFailed('x')
      ..retryStarted()
      ..retryRefused();
    await tester.pumpWidget(rig.build());
    expect(find.textContaining('Nothing to run again'), findsOneWidget);
    expect(find.byKey(const ValueKey('retry-change')), findsNothing);
  });

  testWidgets('a queued Retry shows the change as trying again', (
    tester,
  ) async {
    phone(tester);
    final rig = _Rig();
    rig.tracker
      ..workerWorking(const ['t1'])
      ..workerFailed('x')
      ..retryStarted()
      ..retried();
    await tester.pumpWidget(rig.build());
    expect(find.byKey(const ValueKey('chip-working')), findsOneWidget);
    expect(find.text('Trying again…'), findsOneWidget);
  });

  testWidgets('a failed send offers Retry; a reply opens the threads', (
    tester,
  ) async {
    phone(tester);
    final rig = _Rig();
    rig.tracker.sendFailed('t1', 'Offline');
    await tester.pumpWidget(rig.build());
    await tester.tap(find.byKey(const ValueKey('retry-send')));
    rig.tracker
      ..sent('t2')
      ..workerWorking(const ['t2'])
      ..workerReplied();
    await tester.pumpWidget(rig.build());
    await tester.tap(find.byKey(const ValueKey('chip-replied')));
    await tester.tap(find.byKey(const ValueKey('chip-dismiss')));
    expect(rig.log, ['resend', 'replied', 'dismiss']);
  });

  for (final scale in [1.3, 2.0]) {
    testWidgets('fits a phone at text scale $scale', (tester) async {
      phone(tester);
      final rig = _Rig();
      rig.tracker.workerWorking(const ['t1']);
      rig.narrate([
        'Reading your runs',
        'Changing index.html',
        'Adding the weekly total under the list of runs',
      ]);
      rig.tracker.workerProgress(const ['t1'], Inspected(_png, 1));
      await tester.pumpWidget(rig.build(textScale: scale));
      await tester.tap(find.byKey(const ValueKey('chip-working')));
      await tester.pump();
      expect(tester.takeException(), isNull);
      rig.tracker.workerFailed(
        'the model wrote a tool call that is not valid JSON, twice',
      );
      await tester.pumpWidget(rig.build(textScale: scale));
      expect(tester.takeException(), isNull);
      expect(find.byKey(const ValueKey('retry-change')), findsOneWidget);
    });
  }

  // Renders every state at phone size, light and dark, at three text
  // scales, into $CLAX_RENDER_DIR, to look at; skipped without it.
  final out = Platform.environment['CLAX_RENDER_DIR'];
  group('renders', skip: out == null ? 'set CLAX_RENDER_DIR' : null, () {
    setUpAll(_loadFonts);
    final states = <String, void Function(_Rig)>{
      'starting': (r) => r.tracker
        ..sent('t1')
        ..workerWorking(const ['t1']),
      'step': (r) {
        r.tracker
          ..sent('t1')
          ..workerWorking(const ['t1']);
        r.narrate(['Reading your runs', 'Adding the weekly total']);
        r.clock.t = r.clock.t.add(const Duration(seconds: 12));
      },
      'steps': (r) {
        r.tracker.workerWorking(const ['t1']);
        r.narrate([
          'Reading your runs',
          'Changing index.html',
          'Checking how it looks',
          'Adding the weekly total under the list',
        ]);
        r.tracker.workerProgress(const ['t1'], Inspected(_screenshot!, 1));
        r.clock.t = r.clock.t.add(const Duration(seconds: 23));
      },
      'failed': (r) => r.tracker
        ..workerWorking(const ['t1'])
        ..workerFailed('the connection dropped'),
      'retrying': (r) => r.tracker
        ..workerWorking(const ['t1'])
        ..workerFailed('the connection dropped')
        ..retryStarted(),
      'refused': (r) => r.tracker
        ..workerWorking(const ['t1'])
        ..workerFailed('the connection dropped')
        ..retryStarted()
        ..retryRefused(),
      'again': (r) => r.tracker
        ..workerWorking(const ['t1'])
        ..workerFailed('the connection dropped')
        ..retryStarted()
        ..retried(),
      'landing': (r) {
        r.tracker.workerWorking(const ['t1']);
        r.narrate(['Publishing']);
        r.tracker.workerLanded(4);
      },
    };
    for (final MapEntry(key: name, value: set) in states.entries) {
      for (final b in Brightness.values) {
        for (final scale in [1.0, 1.3, 2.0]) {
          testWidgets('$name ${b.name} $scale', (tester) async {
            phone(tester);
            final rig = _Rig();
            set(rig);
            const key = ValueKey('frame');
            await tester.pumpWidget(
              rig.build(brightness: b, textScale: scale, boundary: key),
            );
            await tester.pump(const Duration(seconds: 1));
            // Let the thumbnail decode.
            await tester.runAsync(
              () => Future<void>.delayed(const Duration(milliseconds: 50)),
            );
            await tester.pump();
            expect(tester.takeException(), isNull);
            final boundary = tester.renderObject<RenderRepaintBoundary>(
              find.byKey(key),
            );
            final bytes = await tester.runAsync(() async {
              final image = await boundary.toImage(pixelRatio: 2);
              final data = await image.toByteData(
                format: ui.ImageByteFormat.png,
              );
              return data!.buffer.asUint8List();
            });
            File('$out/$name-${b.name}-$scale.png')
              ..parent.createSync(recursive: true)
              ..writeAsBytesSync(bytes!);
          });
        }
      }
    }
  });
}

/// A screenshot of the page for the render's thumbnail, drawn once.
Uint8List? _screenshot;

/// Flutter's Roboto and Material Icons, so renders look as on a phone.
Future<void> _loadFonts() async {
  final root = Platform.environment['FLUTTER_ROOT'];
  // The renders are drawn with the test's own fonts where the SDK's are not
  // at hand (a Bazel test has no FLUTTER_ROOT).
  if (root == null) return;
  final dir = '$root/bin/cache/artifacts/material_fonts';
  Future<void> load(String family, List<String> files) async {
    final l = FontLoader(family);
    for (final f in files) {
      final bytes = File('$dir/$f').readAsBytesSync();
      l.addFont(Future.value(ByteData.view(bytes.buffer)));
    }
    await l.load();
  }

  await load('Roboto', ['Roboto-Regular.ttf', 'Roboto-Medium.ttf']);
  await load('MaterialIcons', ['MaterialIcons-Regular.otf']);
  _screenshot = await _drawScreenshot();
}

/// A phone-shaped picture of a page: a title, three rows and a total.
Future<Uint8List> _drawScreenshot() async {
  const w = 360.0, h = 640.0;
  final rec = ui.PictureRecorder();
  final c = Canvas(rec);
  c.drawRect(
    const Rect.fromLTWH(0, 0, w, h),
    Paint()..color = const Color(0xfff7f4ee),
  );
  c.drawRect(
    const Rect.fromLTWH(20, 40, 120, 28),
    Paint()..color = const Color(0xff1d1b16),
  );
  for (var i = 0; i < 3; i++) {
    c.drawRRect(
      RRect.fromRectAndRadius(
        Rect.fromLTWH(20, 96 + i * 60.0, w - 40, 48),
        const Radius.circular(10),
      ),
      Paint()..color = Colors.white,
    );
  }
  c.drawRRect(
    RRect.fromRectAndRadius(
      const Rect.fromLTWH(20, 290, w - 40, 56),
      const Radius.circular(10),
    ),
    Paint()..color = const Color(0xffd8e8d0),
  );
  final image = await rec.endRecording().toImage(w.toInt(), h.toInt());
  final data = await image.toByteData(format: ui.ImageByteFormat.png);
  return data!.buffer.asUint8List();
}
