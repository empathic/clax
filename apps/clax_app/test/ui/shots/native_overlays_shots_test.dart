// The shell's overlays the app draws natively, over an open app, rendered
// light and dark into CLAX_SHOTS_DIR (skipped without it) on the phone, a
// wide window and the Mac: an error notice with Try again
// (`app-notice`), the draft banner while Clax drafts (`app-drafting`) and
// while its draft shows (`app-draft-showing`), and a deleted app
// (`app-gone`).
import 'dart:async';

import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/daemon/clock.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:flutter/material.dart';

import '../../support/fake_surface.dart';
import '../../support/fake_voice.dart';
import 'rig.dart';

/// A clock whose delays never end: a toast stays for the render.
class _Still implements Clock {
  @override
  DateTime now() => DateTime.utc(2026, 10, 19, 9, 41);
  @override
  Future<void> delay(Duration d) => Completer<void>().future;
}

/// A page with a heading at its top, which the draft banner must not
/// cover.
class _Page extends FakeAppSurface {
  @override
  Widget build(BuildContext context) => const ColoredBox(
    key: ValueKey('fake-surface'),
    color: Colors.white,
    child: Padding(
      padding: EdgeInsets.all(20),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            'Runs',
            style: TextStyle(
              color: Color(0xFF102A4C),
              fontSize: 28,
              fontWeight: FontWeight.w700,
              fontFamily: 'Roboto',
            ),
          ),
          SizedBox(height: 12),
          Text(
            'Mon  5.2 km\nWed  8.0 km\nSat  12.4 km',
            style: TextStyle(
              color: Color(0xFF222222),
              fontSize: 16,
              height: 2,
              fontFamily: 'Roboto',
            ),
          ),
        ],
      ),
    ),
  );
}

/// The app over a home route, its page ready, then [then] from the shell.
Future<void> _open(Shot s, String name, List<HostMessage> then) async {
  final api = FakeClaxApi();
  final aid = api.seedArtifact('Runs', {'index.html': '<h1>Runs</h1>'});
  final surface = _Page();
  final nav = GlobalKey<NavigatorState>();
  await s.tester.pumpWidget(
    s.app(
      Navigator(
        key: nav,
        onGenerateRoute: (_) => MaterialPageRoute<void>(
          builder: (_) => const Scaffold(body: SizedBox.expand()),
        ),
      ),
      title: 'Runs',
    ),
  );
  unawaited(
    nav.currentState!.push(
      MaterialPageRoute<void>(
        builder: (_) => AppScreen(
          aid: aid,
          base: Uri.parse('http://127.0.0.1:7999/'),
          api: api,
          surface: surface,
          voice: FakeVoice(),
          settings: AppSettings(),
          timings: const NoTimings(),
          worker: const Stream.empty(),
          clock: _Still(),
        ),
      ),
    ),
  );
  await s.settle();
  surface.push(
    Ready(artifact: aid, version: 3, latest: 3, file: 'index.html'),
  );
  surface.push(const ChromeMsg(Colors.white, Colors.white));
  await s.tester.pump();
  then.forEach(surface.push);
  await s.settle();
  await s.take(name);
  await s.tester.pumpWidget(const SizedBox());
}

void main() {
  const forms = [phone, wide, mac];
  shots(
    'app-notice',
    (s) => _open(s, 'app-notice', const [
      Notice(
        'Couldn’t load the latest version.',
        tone: NoticeTone.error,
        action: (id: 'retry', label: 'Try again'),
      ),
    ]),
    forms: forms,
  );
  shots(
    'app-drafting',
    (s) => _open(s, 'app-drafting', const [DraftMsg(Drafting('Clax', null))]),
    forms: forms,
  );
  shots(
    'app-draft-showing',
    (s) => _open(s, 'app-draft-showing', const [
      DraftMsg(Showing('Clax', 3, 'A bigger heading')),
    ]),
    forms: forms,
  );
  shots(
    'app-drafting',
    (s) => _open(s, 'app-drafting', const [DraftMsg(Drafting('Clax', null))]),
    forms: const [phone],
    textScales: const [2],
  );
  shots(
    'app-gone',
    (s) => _open(s, 'app-gone', const [GoneMsg()]),
    forms: forms,
  );
}
