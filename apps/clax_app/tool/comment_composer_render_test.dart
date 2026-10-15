// Renders the app's comment composer over the app screen to PNGs: a picked
// element with its screenshot, selected text, a drawn area, the whole app,
// dictation, the keyboard up (320, 390 and 1100 dp, text scale 1.3), dark,
// and the threads with their screenshots. Real Roboto and Material Icons
// fonts, over a stand-in page. For looking at, not a gate:
//
//   flutter test tool/comment_composer_render_test.dart
//
// The images go to <repo>/target/flutter-app/c2/ (CLAX_RENDER_DIR overrides).
import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:flutter/foundation.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../test/support/fake_surface.dart';
import '../test/support/fake_voice.dart';

/// A fake surface that draws a stand-in for a small run tracker.
class _PageSurface extends FakeAppSurface {
  @override
  Widget build(BuildContext context) => Container(
    color: const Color(0xFFFFFBF5),
    // A page keeps its own colours whatever the app's theme.
    padding: const EdgeInsets.all(20),
    child: const DefaultTextStyle(
      style: TextStyle(
        fontFamily: 'Roboto',
        color: Color(0xFF221C14),
        fontSize: 15,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            'Runs',
            style: TextStyle(fontSize: 30, fontWeight: FontWeight.w700),
          ),
          SizedBox(height: 4),
          Text('This week: 17 km', style: TextStyle(color: Color(0xFF6B5E4E))),
          SizedBox(height: 20),
          _Run('Mon', '5 km', '27:40'),
          _Run('Wed', '7 km', '39:02'),
          _Run('Sat', '5 km', '26:15'),
        ],
      ),
    ),
  );
}

class _Run extends StatelessWidget {
  const _Run(this.day, this.km, this.time);
  final String day;
  final String km;
  final String time;
  @override
  Widget build(BuildContext context) => Container(
    margin: const EdgeInsets.only(bottom: 10),
    padding: const EdgeInsets.all(14),
    decoration: BoxDecoration(
      color: Colors.white,
      borderRadius: BorderRadius.circular(12),
      border: Border.all(color: const Color(0xFFEADFCF)),
    ),
    child: Row(
      children: [
        Text(day, style: const TextStyle(fontWeight: FontWeight.w600)),
        const Spacer(),
        Text('$km · $time'),
      ],
    ),
  );
}

class _NoTimings implements Timings {
  @override
  void start(String name) {}
  @override
  TimingRecord? stop(String name, {Map<String, Object?> extra = const {}}) =>
      null;
  @override
  List<TimingRecord> recent(int n) => const [];
}

Future<void> _fonts() async {
  final root =
      Platform.environment['FLUTTER_ROOT'] ??
      '/Users/alex/.local/homebrew/share/flutter';
  final dir = '$root/bin/cache/artifacts/material_fonts';
  Future<void> load(String family, List<String> files) async {
    final l = FontLoader(family);
    for (final f in files) {
      final bytes = File('$dir/$f').readAsBytesSync();
      l.addFont(Future.value(ByteData.view(bytes.buffer)));
    }
    await l.load();
  }

  await load('Roboto', [
    'Roboto-Regular.ttf',
    'Roboto-Medium.ttf',
    'Roboto-Bold.ttf',
  ]);
  await load('MaterialIcons', ['MaterialIcons-Regular.otf']);
}

/// A screenshot cut from the stand-in page: a run card, or a line of text.
Future<Uint8List> _clip({required bool text}) async {
  final rec = ui.PictureRecorder();
  final c = Canvas(rec);
  final w = text ? 900.0 : 960.0, h = text ? 90.0 : 150.0;
  c.drawRect(
    Rect.fromLTWH(0, 0, w, h),
    Paint()..color = const Color(0xFFFFFBF5),
  );
  if (!text) {
    final r = RRect.fromRectAndRadius(
      Rect.fromLTWH(6, 6, w - 12, h - 12),
      const Radius.circular(36),
    );
    c.drawRRect(r, Paint()..color = Colors.white);
    c.drawRRect(
      r,
      Paint()
        ..color = const Color(0xFFEADFCF)
        ..style = PaintingStyle.stroke
        ..strokeWidth = 3,
    );
  }
  void label(String s, double x, FontWeight wt) {
    final p = TextPainter(
      text: TextSpan(
        text: s,
        style: TextStyle(
          fontFamily: 'Roboto',
          fontSize: 45,
          fontWeight: wt,
          color: const Color(0xFF221C14),
        ),
      ),
      textDirection: TextDirection.ltr,
    )..layout();
    p.paint(c, Offset(x, (h - p.height) / 2));
  }

  if (text) {
    label('This week: 17 km', 12, FontWeight.w400);
  } else {
    label('Mon', 48, FontWeight.w600);
    label('5 km · 27:40', w - 330, FontWeight.w400);
  }
  final img = await rec.endRecording().toImage(w.toInt(), h.toInt());
  return (await img.toByteData(format: ui.ImageByteFormat.png))!.buffer
      .asUint8List();
}

void main() {
  final out = Directory(
    Platform.environment['CLAX_RENDER_DIR'] ?? '../../target/flutter-app/c2',
  )..createSync(recursive: true);
  setUpAll(_fonts);

  Future<void> shot(WidgetTester tester, String name) async {
    for (var i = 0; i < 6; i++) {
      await tester.pump(const Duration(milliseconds: 100));
    }
    final boundary = tester.renderObject(
      find.byKey(const ValueKey('shot')),
    ) as RenderRepaintBoundary;
    await tester.runAsync(() async {
      final image = await boundary.toImage(
        pixelRatio: tester.view.devicePixelRatio,
      );
      final png = await image.toByteData(format: ui.ImageByteFormat.png);
      File('${out.path}/$name.png').writeAsBytesSync(png!.buffer.asUint8List());
    });
  }

  Future<(FakeAppSurface, FakeClaxApi, String)> pump(
    WidgetTester tester, {
    Size size = const Size(390, 844),
    Brightness brightness = Brightness.light,
    double scale = 1.0,
    bool dictate = false,
  }) async {
    tester.view.devicePixelRatio = 3;
    tester.view.physicalSize = size * 3;
    tester.view.padding = const FakeViewPadding(top: 72, bottom: 48);
    addTearDown(tester.view.reset);
    tester.platformDispatcher.textScaleFactorTestValue = scale;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    final api = FakeClaxApi(clock: () => '2026-10-15T09:14:00Z');
    final aid = api.seedArtifact('Runs', {'index.html': '<h1>Runs</h1>'});
    await api.registerAppSession(installId: 'i', pid: 1);
    final surface = _PageSurface();
    final worker = StreamController<WorkerEvent>.broadcast(sync: true);
    addTearDown(worker.close);
    ThemeData theme(Brightness b) => claxTheme(
      b,
    ).copyWith(textTheme: claxTheme(b).textTheme.apply(fontFamily: 'Roboto'));
    await tester.pumpWidget(
      RepaintBoundary(
        key: const ValueKey('shot'),
        child: MaterialApp(
          debugShowCheckedModeBanner: false,
          theme: theme(Brightness.light),
          darkTheme: theme(Brightness.dark),
          themeMode: brightness == Brightness.dark
              ? ThemeMode.dark
              : ThemeMode.light,
          home: AppScreen(
            aid: aid,
            base: Uri.parse('http://127.0.0.1:7999'),
            api: api,
            surface: surface,
            voice: FakeVoice(
              script: const ['make the cards', 'make the cards show the pace'],
              hold: true,
            ),
            settings: AppSettings(dictateComments: dictate),
            timings: _NoTimings(),
            worker: worker.stream,
          ),
        ),
      ),
    );
    surface.push(
      Ready(artifact: aid, version: 3, latest: 3, file: 'index.html'),
    );
    await tester.pump();
    return (surface, api, aid);
  }

  Future<void> compose(
    WidgetTester tester,
    FakeAppSurface s,
    Draft d,
    Uint8List? png,
  ) async {
    s.push(StateMsg(commenting: false, draft: d, busy: false));
    await tester.pump();
    if (png != null) {
      s.push(DraftClipMsg(d.key, png));
      await tester.pump();
      await tester.runAsync(
        () => precacheImage(
          MemoryImage(png),
          tester.element(find.byType(AppScreen)),
        ),
      );
    }
    await tester.pump();
  }

  Future<void> type(WidgetTester tester, String text) async {
    await tester.enterText(find.byKey(const ValueKey('composer-field')), text);
    await tester.pump();
  }

  void keyboard(WidgetTester tester) {
    tester.view.viewInsets = const FakeViewPadding(bottom: 270 * 3);
    addTearDown(tester.view.resetViewInsets);
  }

  const element = Draft(
    key: 'p1',
    kind: 'element',
    label: '“Mon 5 km · 27:40”',
    quote: 'Mon 5 km · 27:40',
    text: '',
    clip: DraftClip.ready,
  );
  const range = Draft(
    key: 'r1',
    kind: 'range',
    label: '“This week: 17 km”',
    quote: 'This week: 17 km, and a longer run planned for Sunday morning along the river',
    text: '',
    clip: DraftClip.ready,
  );
  const area = Draft(
    key: 'a1',
    kind: 'area',
    label: 'this area',
    text: '',
    clip: DraftClip.ready,
  );
  const whole = Draft(
    key: 'w1',
    kind: 'artifact',
    label: 'The whole app',
    text: '',
    clip: DraftClip.none,
  );

  testWidgets('element pick with its screenshot', (tester) async {
    final (s, _, _) = await pump(tester);
    final png = (await tester.runAsync(() => _clip(text: false)))!;
    await compose(tester, s, element, png);
    await shot(tester, '01-element-390');
    keyboard(tester);
    await type(tester, 'Show the pace per km under the distance');
    await shot(tester, '02-element-keyboard-390');
  });

  testWidgets('element pick, screenshot still coming', (tester) async {
    final (s, _, _) = await pump(tester);
    await compose(
      tester,
      s,
      const Draft(
        key: 'p2',
        label: '“Mon 5 km · 27:40”',
        quote: 'Mon 5 km · 27:40',
        text: '',
        clip: DraftClip.pending,
      ),
      null,
    );
    await shot(tester, '03-element-pending-390');
  });

  testWidgets('selected text', (tester) async {
    final (s, _, _) = await pump(tester);
    final png = (await tester.runAsync(() => _clip(text: true)))!;
    await compose(tester, s, range, png);
    await shot(tester, '04-range-390');
  });

  testWidgets('drawn area', (tester) async {
    final (s, _, _) = await pump(tester);
    final png = (await tester.runAsync(() => _clip(text: false)))!;
    await compose(tester, s, area, png);
    await shot(tester, '05-area-390');
  });

  testWidgets('whole app', (tester) async {
    final (s, _, _) = await pump(tester);
    await compose(tester, s, whole, null);
    await type(tester, 'Use a darker theme throughout');
    await shot(tester, '06-whole-app-390');
  });

  testWidgets('dictating', (tester) async {
    final (s, _, _) = await pump(tester, dictate: true);
    final png = (await tester.runAsync(() => _clip(text: false)))!;
    await compose(tester, s, element, png);
    await shot(tester, '07-dictating-390');
  });

  testWidgets('320 dp, keyboard up, text scale 1.3', (tester) async {
    final (s, _, _) = await pump(
      tester,
      size: const Size(320, 568),
      scale: 1.3,
    );
    final png = (await tester.runAsync(() => _clip(text: false)))!;
    await compose(tester, s, element, png);
    keyboard(tester);
    await type(tester, 'Show the pace per km');
    await shot(tester, '08-keyboard-320-scale1.3');
  });

  testWidgets('390 dp, keyboard up, dark', (tester) async {
    final (s, _, _) = await pump(tester, brightness: Brightness.dark);
    final png = (await tester.runAsync(() => _clip(text: true)))!;
    await compose(tester, s, range, png);
    keyboard(tester);
    await type(tester, 'Say 17.0 km');
    await shot(tester, '09-range-keyboard-dark-390');
  });

  testWidgets(
    'wide: the composer heads the side panel, threads with screenshots',
    (tester) async {
      final (s, api, aid) = await pump(
        tester,
        size: const Size(1100, 720),
        scale: 1.3,
      );
      final png = (await tester.runAsync(() => _clip(text: false)))!;
      final t = api
          .sendComment(aid, 'Make the run cards bigger', clip: png)
          .threadId;
      s.push(
        ThreadsMsg([
          ThreadSummary(
            id: t,
            status: 'open',
            summary: 'Make the run cards bigger',
            label: '“Mon 5 km · 27:40”',
            sent: true,
            comments: 1,
            version: 3,
            clip: true,
          ),
        ]),
      );
      await compose(tester, s, element, png);
      await type(tester, 'Show the pace per km');
      await shot(tester, '10-wide-1100-scale1.3');
    },
  );

  testWidgets('threads with their screenshots', (tester) async {
    final (s, api, aid) = await pump(tester);
    final png = (await tester.runAsync(() => _clip(text: false)))!;
    final t1 = api
        .sendComment(aid, 'Make the run cards bigger', clip: png)
        .threadId;
    final t2 = api.sendComment(aid, 'Use a darker theme').threadId;
    s.push(
      ThreadsMsg([
        ThreadSummary(
          id: t1,
          status: 'open',
          summary: 'Make the run cards bigger and show the pace',
          label: '“Mon 5 km · 27:40”',
          sent: true,
          comments: 1,
          version: 3,
          clip: true,
        ),
        ThreadSummary(
          id: t2,
          status: 'open',
          summary: 'Use a darker theme',
          label: 'Whole app',
          sent: true,
          comments: 1,
          version: 3,
        ),
      ]),
    );
    // A two-finger swipe left opens the threads.
    final a = await tester.startGesture(const Offset(200, 300), pointer: 1);
    final b = await tester.startGesture(const Offset(200, 400), pointer: 2);
    for (var i = 1; i <= 3; i++) {
      await a.moveTo(Offset(200 - 40.0 * i, 300));
      await b.moveTo(Offset(200 - 40.0 * i, 400));
    }
    await a.up();
    await b.up();
    await tester.pump();
    await tester.runAsync(() async {
      await Future<void>.delayed(const Duration(milliseconds: 20));
      await precacheImage(
        MemoryImage(png),
        tester.element(find.byType(AppScreen)),
      );
    });
    await shot(tester, '11-threads-390');
    expect(kDebugMode, isTrue);
  });
}
