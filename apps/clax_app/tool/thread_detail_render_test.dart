// Renders the thread page and the composer peek to PNGs at phone size
// (360 x 780 and 320 x 640 at a device pixel ratio of 3), light and dark,
// at text scales 1.0, 1.3 and 2.0, with the real Roboto and Material Icons
// fonts. For looking at, not a gate:
//
//   flutter test tool/thread_detail_render_test.dart
//
// The images go to <repo>/target/u4-render/ (CLAX_RENDER_DIR overrides).
// Each render fails on a layout overflow.
import 'dart:io';
import 'dart:ui' as ui;

import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/thread_detail.dart';
import 'package:clax_app/ui/threads_panel.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../test/support/fake_voice.dart';

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

/// A clip: a run card cut from the page, drawn here.
Future<Uint8List> _clipPng() async {
  final rec = ui.PictureRecorder();
  final c = Canvas(rec);
  const w = 600.0, h = 150.0;
  c.drawRect(
    const Rect.fromLTWH(0, 0, w, h),
    Paint()..color = const Color(0xFFFFFBF5),
  );
  c.drawRRect(
    RRect.fromRectAndRadius(
      const Rect.fromLTWH(24, 24, w - 48, h - 48),
      const Radius.circular(24),
    ),
    Paint()..color = Colors.white,
  );
  c.drawRRect(
    RRect.fromRectAndRadius(
      const Rect.fromLTWH(24, 24, w - 48, h - 48),
      const Radius.circular(24),
    ),
    Paint()
      ..color = const Color(0xFFE07A2F)
      ..style = PaintingStyle.stroke
      ..strokeWidth = 4,
  );
  final p = TextPainter(
    text: const TextSpan(
      text: 'Mon                         5 km · 27:40',
      style: TextStyle(
        fontFamily: 'Roboto',
        fontSize: 30,
        color: Color(0xFF221C14),
      ),
    ),
    textDirection: TextDirection.ltr,
  )..layout();
  p.paint(c, const Offset(56, 56));
  final img = await rec.endRecording().toImage(w.toInt(), h.toInt());
  final bytes = await img.toByteData(format: ui.ImageByteFormat.png);
  return bytes!.buffer.asUint8List();
}

ThemeData _theme(Brightness b) => claxTheme(b)
    .copyWith(textTheme: claxTheme(b).textTheme.apply(fontFamily: 'Roboto'));

void main() {
  final out = Directory(
    Platform.environment['CLAX_RENDER_DIR'] ?? '../../target/u4-render',
  )..createSync(recursive: true);

  setUpAll(_fonts);

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

  Future<void> frame(
    WidgetTester tester,
    Widget child, {
    required Brightness brightness,
    required double scale,
    Size size = const Size(360, 780),
  }) async {
    tester.view.devicePixelRatio = 3;
    tester.view.physicalSize = size * 3;
    tester.view.padding = const FakeViewPadding(top: 72, bottom: 48);
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      RepaintBoundary(
        key: const ValueKey('shot'),
        child: MaterialApp(
          debugShowCheckedModeBanner: false,
          theme: _theme(Brightness.light),
          darkTheme: _theme(Brightness.dark),
          themeMode: brightness == Brightness.dark
              ? ThemeMode.dark
              : ThemeMode.light,
          builder: (context, child) => MediaQuery.withClampedTextScaling(
            minScaleFactor: scale,
            maxScaleFactor: scale,
            child: child!,
          ),
          home: child,
        ),
      ),
    );
  }

  /// A thread as it stands after [kind]: `replied` (a conversation with a
  /// clip and a version), `failed` or `unsent`.
  Future<(FakeClaxApi, String, String)> seeded(String kind) async {
    final api = FakeClaxApi(clock: () => '2026-10-12T09:02:00Z');
    final aid = api.seedArtifact('Runs', {'index.html': '<h1>Runs</h1>'});
    await api.registerAppSession(installId: 'i', pid: 1);
    final anchor = {
      'kind': 'element',
      'file': 'index.html',
      'selector': 'main > div.run:nth-of-type(1)',
    };
    if (kind == 'unsent') {
      final tid = api.addThread(
        aid,
        'The Monday card should show my pace too',
        anchor: anchor,
      );
      return (api, aid, tid);
    }
    final item = api.sendComment(
      aid,
      'Make the Monday card show my pace per kilometre',
      anchor: anchor,
      clip: await _clipOnce,
    );
    final tid = item.threadId;
    if (kind == 'failed') {
      await api.replyAsAgent(
        aid,
        tid,
        "Couldn't make that change: the connection to the model dropped "
        'before it finished.',
      );
      return (api, aid, tid);
    }
    await api.publish(
      aid,
      PublishRequest(
        ifVersion: 1,
        note: 'Pace on each run card',
        files: {'index.html': 'x'},
        addresses: [tid],
      ),
    );
    await api.replyAsAgent(
      aid,
      tid,
      'Made this change in version 2: each run card shows the pace '
      '(5:32 /km on Monday).',
    );
    await api.postOwnerComment(aid, tid, 'Nice. Can it be bold?');
    await api.publish(
      aid,
      PublishRequest(
        ifVersion: 2,
        note: 'Bold pace',
        files: {'index.html': 'y'},
        addresses: [tid],
      ),
    );
    await api.replyAsAgent(aid, tid, 'Made this change in version 3.');
    return (api, aid, tid);
  }

  for (final kind in ['replied', 'failed', 'unsent']) {
    for (final brightness in Brightness.values) {
      for (final scale in [1.0, 1.3, 2.0]) {
        final name = 'thread-$kind-${brightness.name}-$scale';
        testWidgets(name, (tester) async {
          late (FakeClaxApi, String, String) s;
          await tester.runAsync(() async => s = await seeded(kind));
          final (api, aid, tid) = s;
          await frame(
            tester,
            ThreadDetail(
              api: api,
              aid: aid,
              tid: tid,
              voice: FakeVoice(),
              autoDictate: false,
              label: 'the run card “Mon”',
              onPreview: (_) {},
              onRetry: (_) async => true,
              now: () => DateTime.utc(2026, 10, 12, 9, 14),
            ),
            brightness: brightness,
            scale: scale,
          );
          await tester.runAsync(
            () => Future<void>.delayed(const Duration(milliseconds: 50)),
          );
          await shot(tester, name);
        });
      }
    }
  }

  for (final brightness in Brightness.values) {
    for (final scale in [1.0, 1.3, 2.0]) {
      final name = 'peek-320-${brightness.name}-$scale';
      testWidgets(name, (tester) async {
        await frame(
          tester,
          Scaffold(
            body: Column(
              children: [
                const Expanded(child: ColoredBox(color: Color(0xFFFFFBF5))),
                SizedBox(
                  height: AppPanel.peekHeight + 48,
                  child: AppPanel(
                    wide: false,
                    tab: PanelTab.threads,
                    onTab: (_) {},
                    onClose: () {},
                    draft: const Draft(
                      label: 'The Mon run card',
                      text: '',
                      clip: DraftClip.ready,
                    ),
                    busy: false,
                    dictation: const Dictation(
                      listening: true,
                      words:
                          'make the Monday card show the pace per kilometre '
                          'and the heart rate under it',
                    ),
                    onMic: () {},
                    threads: const [],
                    onThread: (_) {},
                    onResend: (_) {},
                    versions: const SizedBox(),
                  ),
                ),
              ],
            ),
          ),
          brightness: brightness,
          scale: scale,
          size: const Size(320, 640),
        );
        await shot(tester, name);
      });
    }
  }
}

/// The clip, drawn once.
final Future<Uint8List> _clipOnce = _clipPng();
