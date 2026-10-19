// An open app on the phone, rendered light and dark into CLAX_SHOTS_DIR
// (skipped without it), drawn edge to edge: a light page whose header is
// dark blue and whose tab bar is white (`app`), its status band in the
// header's blue and its navigation band in the tab bar's white; and a dark
// page, black at the top and charcoal at the bottom (`app-dark-page`). The status and gesture bar
// stand-ins draw their icons in the brightness the screen's bands ask the
// system for (`SystemChrome.latestStyle`), not the theme's.
import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../support/fake_surface.dart';
import '../../support/fake_voice.dart';
import 'rig.dart';

const _blue = Color(0xFF102A4C);

/// The surface's page: a header in [head], rows in [ink] on [body], and a
/// tab bar in [tabs].
class _PageSurface extends FakeAppSurface {
  _PageSurface({
    this.head = _blue,
    this.body = Colors.white,
    this.ink = const Color(0xFF222222),
    this.tabs = Colors.white,
  });
  final Color head, body, ink, tabs;

  @override
  Widget build(BuildContext context) => ColoredBox(
    key: const ValueKey('fake-surface'),
    color: body,
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        ColoredBox(
          color: head,
          child: const Padding(
            padding: EdgeInsets.fromLTRB(20, 16, 20, 16),
            child: Text(
              'Runs',
              style: TextStyle(
                color: Colors.white,
                fontSize: 22,
                fontWeight: FontWeight.w600,
                fontFamily: 'Roboto',
              ),
            ),
          ),
        ),
        Expanded(
          child: Padding(
            padding: const EdgeInsets.all(20),
            child: Text(
              'Mon  5.2 km\nWed  8.0 km\nSat  12.4 km',
              style: TextStyle(
                color: ink,
                fontSize: 16,
                height: 2,
                fontFamily: 'Roboto',
              ),
            ),
          ),
        ),
        DecoratedBox(
          decoration: BoxDecoration(
            color: tabs,
            border: const Border(top: BorderSide(color: Color(0x33888888))),
          ),
          child: const SizedBox(
            height: 56,
            child: Row(
              mainAxisAlignment: MainAxisAlignment.spaceAround,
              children: [
                Icon(Icons.list, color: _blue),
                Icon(Icons.bar_chart, color: Color(0xFF888888)),
                Icon(Icons.settings, color: Color(0xFF888888)),
              ],
            ),
          ),
        ),
      ],
    ),
  );
}

/// Stand-ins for the status bar's icons and the gesture bar's handle, in
/// the brightness the app last asked the system for.
class _Bars extends StatelessWidget {
  const _Bars(this.style, this.form);
  final ValueListenable<SystemUiOverlayStyle?> style;
  final ShotForm form;

  static Color _ink(Brightness? icons) =>
      icons == Brightness.light ? Colors.white : Colors.black;

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: style,
    builder: (context, s, _) {
      final top = _ink(s?.statusBarIconBrightness);
      final bottom = _ink(s?.systemNavigationBarIconBrightness);
      return Stack(
        children: [
          Positioned(
            left: 0,
            right: 0,
            top: 0,
            height: form.top,
            child: DefaultTextStyle(
              style: TextStyle(fontSize: 13, color: top, fontFamily: 'Roboto'),
              child: Row(
                children: [
                  const SizedBox(width: 18),
                  const Text('9:41'),
                  const Spacer(),
                  Icon(Icons.signal_cellular_alt, size: 14, color: top),
                  Icon(Icons.wifi, size: 14, color: top),
                  Icon(Icons.battery_full, size: 14, color: top),
                  const SizedBox(width: 14),
                ],
              ),
            ),
          ),
          Positioned(
            left: 0,
            right: 0,
            bottom: (form.bottom - 4) / 2,
            child: Center(
              child: Container(
                width: 108,
                height: 4,
                decoration: BoxDecoration(
                  color: bottom.withValues(alpha: 0.75),
                  borderRadius: BorderRadius.circular(2),
                ),
              ),
            ),
          ),
        ],
      );
    },
  );
}

Future<void> _open(
  Shot s,
  String surfaceName,
  _PageSurface surface,
  ChromeMsg chrome, {
  required Brightness status,
  required Brightness navigation,
}) async {
    final api = FakeClaxApi();
    final aid = api.seedArtifact('Runs', {'index.html': '<h1>Runs</h1>'});
    final style = ValueNotifier<SystemUiOverlayStyle?>(null);
    addTearDown(style.dispose);
    await s.tester.pumpWidget(
      s.app(
        chrome: false,
        Stack(
          children: [
            AppScreen(
              aid: aid,
              base: Uri.parse('http://127.0.0.1:7999/'),
              api: api,
              surface: surface,
              voice: FakeVoice(),
              settings: AppSettings(),
              timings: const NoTimings(),
              worker: const Stream.empty(),
            ),
            IgnorePointer(child: _Bars(style, s.form)),
          ],
        ),
      ),
    );
    surface.push(
      const Ready(artifact: 'x', version: 1, latest: 1, file: 'index.html'),
    );
    surface.push(chrome);
    await s.settle();
    style.value = SystemChrome.latestStyle;
    await s.tester.pump();
    expect(style.value!.statusBarIconBrightness, status);
    expect(style.value!.systemNavigationBarIconBrightness, navigation);
    await s.take(surfaceName);
}

void main() {
  shots(
    'app',
    (s) => _open(
      s,
      'app',
      _PageSurface(),
      const ChromeMsg(_blue, Colors.white),
      status: Brightness.light,
      navigation: Brightness.dark,
    ),
    forms: const [phone],
  );
  const black = Color(0xFF000000), charcoal = Color(0xFF1E1E1E);
  shots(
    'app-dark-page',
    (s) => _open(
      s,
      'app-dark-page',
      _PageSurface(
        head: black,
        body: const Color(0xFF121212),
        ink: const Color(0xFFE6E6E6),
        tabs: charcoal,
      ),
      const ChromeMsg(black, charcoal),
      status: Brightness.light,
      navigation: Brightness.light,
    ),
    forms: const [phone],
  );
}
