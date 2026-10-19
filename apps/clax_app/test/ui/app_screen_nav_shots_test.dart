// Renders the folded Working pill and the location pill under a tall status
// bar and under a punch-hole cutout, and the New version toast, for a person
// to look at. A stand-in status bar (clock, cutout, icons) is drawn over the
// top inset. Off by default; run with
//   CLAX_NAV_SHOTS=1 flutter test --update-goldens test/ui/app_screen_nav_shots_test.dart
// and look at target/flutter-app/nav/*.png (or CLAX_NAV_SHOTS_OUT, a
// directory relative to this test's directory).
import 'dart:async';
import 'dart:io';

import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/daemon/clock.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_surface.dart';
import '../support/fake_voice.dart';

final _on = Platform.environment['CLAX_NAV_SHOTS'] == '1';
final _out =
    Platform.environment['CLAX_NAV_SHOTS_OUT'] ??
    '../../../../target/flutter-app/nav';

Future<void> _fonts() async {
  final root = Platform.environment['FLUTTER_ROOT']!;
  final dir = '$root/bin/cache/artifacts/material_fonts';
  Future<ByteData> f(String n) async =>
      ByteData.sublistView(await File('$dir/$n').readAsBytes());
  await (FontLoader('Roboto')
        ..addFont(f('Roboto-Regular.ttf'))
        ..addFont(f('Roboto-Medium.ttf'))
        ..addFont(f('Roboto-Bold.ttf')))
      .load();
  await (FontLoader(
    'MaterialIcons',
  )..addFont(f('MaterialIcons-Regular.otf'))).load();
}

/// A page with an app bar of its own at the top, as a kit app has.
class _PageSurface extends FakeAppSurface {
  @override
  Widget build(BuildContext context) => ColoredBox(
    color: const Color(0xFFF6F3EE),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Container(
          height: 56,
          color: const Color(0xFF2F5D50),
          padding: const EdgeInsets.symmetric(horizontal: 16),
          child: const Row(
            children: [
              Expanded(
                child: Text(
                  'My runs',
                  style: TextStyle(color: Colors.white, fontSize: 20),
                ),
              ),
              Icon(Icons.add, color: Colors.white),
              SizedBox(width: 16),
              Icon(Icons.more_vert, color: Colors.white),
            ],
          ),
        ),
        const Padding(
          padding: EdgeInsets.all(16),
          child: Text('Mon  5 km\nTue  3 km\nThu  8 km'),
        ),
      ],
    ),
  );
}

/// The system's status bar over the top inset: the clock, a cutout and
/// the icons, as a phone draws them.
class _StatusBar extends StatelessWidget {
  const _StatusBar({required this.height, required this.cutout});
  final double height;
  final bool cutout;

  @override
  Widget build(BuildContext context) => SizedBox(
    height: height,
    child: Stack(
      children: [
        const Positioned(
          left: 20,
          bottom: 6,
          child: Text(
            '9:41',
            style: TextStyle(fontSize: 14, fontFamily: 'Roboto'),
          ),
        ),
        if (cutout)
          Align(
            alignment: Alignment.center,
            child: Container(
              width: 26,
              height: 26,
              decoration: const BoxDecoration(
                color: Colors.black,
                shape: BoxShape.circle,
              ),
            ),
          ),
        const Positioned(
          right: 16,
          bottom: 6,
          child: Row(
            children: [
              Icon(Icons.signal_cellular_alt, size: 16),
              Icon(Icons.wifi, size: 16),
              Icon(Icons.battery_full, size: 16),
            ],
          ),
        ),
      ],
    ),
  );
}

class _Clock implements Clock {
  @override
  DateTime now() => DateTime.utc(2026, 10, 15, 9);
  final _delays = <Completer<void>>[];
  @override
  Future<void> delay(Duration d) =>
      (_delays..add(Completer<void>())).last.future;
  void lapse() {
    for (final c in List.of(_delays)) {
      c.complete();
    }
    _delays.clear();
  }
}

void main() {
  for (final (name, top, cutout) in const [
    ('tall-status-bar', 48.0, false),
    ('cutout', 56.0, true),
  ]) {
    testWidgets('pill under $name', skip: !_on, (tester) async {
      await tester.runAsync(_fonts);
      tester.view.physicalSize = const Size(360, 400) * 3;
      tester.view.devicePixelRatio = 3;
      tester.view.padding = FakeViewPadding(top: top * 3);
      addTearDown(tester.view.reset);
      final api = FakeClaxApi();
      final aid = api.seedArtifact('Runs', {'index.html': '<h1>Runs</h1>'});
      final surface = _PageSurface();
      final clock = _Clock();
      await tester.pumpWidget(
        MaterialApp(
          debugShowCheckedModeBanner: false,
          theme: claxTheme(Brightness.light, defaultTargetPlatform),
          builder: (context, child) => Stack(
            children: [
              child!,
              _StatusBar(height: top, cutout: cutout),
            ],
          ),
          home: AppScreen(
            aid: aid,
            base: Uri.parse('http://127.0.0.1:7999/'),
            api: api,
            surface: surface,
            voice: FakeVoice(),
            settings: AppSettings(),
            timings: const NoTimings(),
            worker: const Stream.empty(),
            clock: clock,
          ),
        ),
      );
      surface.push(
        const Ready(artifact: '', version: 1, latest: 1, file: 'index.html'),
      );
      final tid = api.sendComment(aid, 'bigger').threadId;
      surface.push(Sent(tid));
      await tester.pump();
      clock.lapse();
      await tester.pump();
      await expectLater(
        find.byType(MaterialApp),
        matchesGoldenFile('$_out/pill-$name.png'),
      );
    });
  }

  testWidgets('the New version toast', skip: !_on, (tester) async {
    await tester.runAsync(_fonts);
    tester.view.physicalSize = const Size(360, 640) * 3;
    tester.view.devicePixelRatio = 3;
    tester.view.padding = const FakeViewPadding(top: 24 * 3, bottom: 16 * 3);
    addTearDown(tester.view.reset);
    final api = FakeClaxApi();
    final aid = api.seedArtifact('Runs', {'index.html': '<h1>Runs</h1>'});
    final surface = _PageSurface();
    await tester.pumpWidget(
      MaterialApp(
        debugShowCheckedModeBanner: false,
        theme: claxTheme(Brightness.light, defaultTargetPlatform),
        home: AppScreen(
          aid: aid,
          base: Uri.parse('http://127.0.0.1:7999/'),
          api: api,
          surface: surface,
          voice: FakeVoice(),
          settings: AppSettings(),
          timings: const NoTimings(),
          worker: const Stream.empty(),
          clock: _Clock(),
        ),
      ),
    );
    surface.push(
      const Ready(artifact: '', version: 1, latest: 1, file: 'index.html'),
    );
    surface.push(const NewerMsg(2));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 600));
    await expectLater(
      find.byType(MaterialApp),
      matchesGoldenFile('$_out/newer-toast.png'),
    );
  });
}
