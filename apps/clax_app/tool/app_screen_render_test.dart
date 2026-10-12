// Renders the app screen's states to PNGs at phone size (360 x 780 at a
// device pixel ratio of 3) and one wide window, with the real Roboto and
// Material Icons fonts, over a stand-in page. For looking at, not a gate:
//
//   flutter test tool/app_screen_render_test.dart
//
// The images go to <repo>/target/f8-render/ (CLAX_RENDER_DIR overrides).
import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/app_surface.dart';
import 'package:clax_app/ui/host_protocol.dart';
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

void main() {
  final out = Directory(
    Platform.environment['CLAX_RENDER_DIR'] ?? '../../target/f8-render',
  )..createSync(recursive: true);

  setUpAll(_fonts);

  // Spinners never settle: pump a fixed run of frames instead.
  Future<void> shot(WidgetTester tester, String name) async {
    for (var i = 0; i < 8; i++) {
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

  Future<(FakeAppSurface, StreamController<WorkerEvent>, String)> pump(
    WidgetTester tester, {
    Size size = const Size(360, 780),
    Brightness brightness = Brightness.light,
    FakeVoice? voice,
  }) async {
    tester.view.devicePixelRatio = 3;
    tester.view.physicalSize = size * 3;
    tester.view.padding = const FakeViewPadding(top: 72, bottom: 48);
    addTearDown(tester.view.reset);
    final api = FakeClaxApi(clock: () => '2026-10-12T09:14:00Z');
    final aid = api.seedArtifact('Runs', {'index.html': '<h1>Runs</h1>'});
    await api.registerAppSession(installId: 'i', pid: 1);
    await api.publish(
      aid,
      PublishRequest(
        ifVersion: 1,
        note: 'Weekly total under the title',
        files: {'index.html': 'x'},
      ),
    );
    await api.publish(
      aid,
      PublishRequest(
        ifVersion: 2,
        note: 'Bigger run cards',
        files: {'index.html': 'y'},
      ),
    );
    final surface = _PageSurface();
    final worker = StreamController<WorkerEvent>.broadcast(sync: true);
    await tester.pumpWidget(
      RepaintBoundary(
        key: const ValueKey('shot'),
        child: MaterialApp(
          debugShowCheckedModeBanner: false,
          theme: claxTheme(Brightness.light).copyWith(
            textTheme: claxTheme(Brightness.light).textTheme
                .apply(fontFamily: 'Roboto'),
          ),
          darkTheme: claxTheme(Brightness.dark).copyWith(
            textTheme: claxTheme(Brightness.dark).textTheme
                .apply(fontFamily: 'Roboto'),
          ),
          themeMode: brightness == Brightness.dark
              ? ThemeMode.dark
              : ThemeMode.light,
          home: AppScreen(
            aid: aid,
            base: Uri.parse('http://127.0.0.1:7999'),
            api: api,
            surface: surface,
            voice:
                voice ??
                FakeVoice(
                  script: const ['make the cards', 'make the cards show pace'],
                  hold: true,
                ),
            settings: AppSettings(),
            timings: _NoTimings(),
            worker: worker.stream,
          ),
        ),
      ),
    );
    return (surface, worker, aid);
  }

  const threads = ThreadsMsg([
    ThreadSummary(
      id: 't1',
      status: 'resolved',
      summary: 'Show the weekly total under the title',
      sent: true,
      comments: 2,
      version: 1,
    ),
    ThreadSummary(
      id: 't2',
      status: 'open',
      summary: 'Make the run cards bigger and show the pace per km',
      label: 'The run cards',
      sent: true,
      comments: 1,
      version: 3,
    ),
  ]);
  Ready ready(String aid) =>
      Ready(artifact: aid, version: 3, latest: 3, file: 'index.html');

  for (final b in Brightness.values) {
    testWidgets('render ${b.name}', (tester) async {
      final (s, worker, aid) = await pump(tester, brightness: b);
      await shot(tester, 'loading_${b.name}');
      s.push(ready(aid));
      await shot(tester, 'page_${b.name}');
      await tester.tap(find.byKey(const ValueKey('panel-handle')));
      s.push(threads);
      await shot(tester, 'threads_${b.name}');
      await tester.tap(find.text('Versions'));
      await shot(tester, 'versions_${b.name}');
      s.push(
        const StateMsg(
          commenting: true,
          draft: Draft(
            label: 'The Mon run card',
            text: '',
            clip: DraftClip.ready,
          ),
          busy: false,
        ),
      );
      await tester.pump();
      await tester.pump();
      await shot(tester, 'peek_${b.name}');
      s.push(const Sent('t3'));
      s.push(const StateMsg(commenting: true, draft: null, busy: false));
      await tester.pump();
      await tester.tap(find.byTooltip('Close'));
      await shot(tester, 'working_${b.name}');
      worker.add(WorkerFailed(aid, 'The API key was refused'));
      await shot(tester, 'failed_${b.name}');
      worker.add(WorkerCancelled(aid));
      await shot(tester, 'stopped_${b.name}');
      worker.add(WorkerReplied(aid));
      await shot(tester, 'replied_${b.name}');
      s.fault(
        const SurfaceFault('The daemon did not answer', reloading: false),
      );
      await shot(tester, 'gaveup_${b.name}');
    });
  }

  testWidgets('render denied mic', (tester) async {
    final (s, _, aid) = await pump(
      tester,
      voice: FakeVoice.denied('Microphone access is turned off for Clax'),
    );
    s.push(ready(aid));
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('panel-handle')));
    s.push(
      const StateMsg(
        commenting: true,
        draft: Draft(label: 'h1', text: '', clip: DraftClip.pending),
        busy: false,
      ),
    );
    await shot(tester, 'peek_denied');
  });

  testWidgets('render wide', (tester) async {
    final (s, _, aid) = await pump(tester, size: const Size(1100, 720));
    s.push(ready(aid));
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('panel-handle')));
    s.push(threads);
    await shot(tester, 'wide_threads');
    await tester.tap(find.text('Versions'));
    await shot(tester, 'wide_versions');
  });
}
