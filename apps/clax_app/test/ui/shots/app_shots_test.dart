// An open app on the phone, rendered light and dark into CLAX_SHOTS_DIR
// (skipped without it), drawn edge to edge: a light page whose header is
// dark blue and whose tab bar is white (`app`), its status band in the
// header's blue and its navigation band in the tab bar's white; a dark
// page, black at the top and charcoal at the bottom (`app-dark-page`). On
// the light page, the Clax pill in each state, at 100% and 200% text: a
// change under way (`app-working`), folded to the mark after its 3 s
// (`app-working-folded`), the build (`app-building`), a new version once
// its toast went (`app-new-version`) and location in use
// (`app-location`); the New version toast at the top (`app-toast`); the
// first-run hint with Turn on (`app-hint`); the Comment tab with
// notifications off, 24 dp inside a 24 dp back-gesture zone
// (`app-comment-tab`), and its menu open over a change under way
// (`app-comment-tab-menu`); and the Rally of 10 a moment into its rally
// (`app-rally`). On the dark page, the Comment tab too
// (`app-comment-tab-dark-page`). Comment mode, on the phone and the Mac:
// its bar under the page (`app-comment-bar`, and at 340 dp,
// `app-comment-bar-narrow`), the bar while the page is slow
// (`app-comment-waiting`), a pick's composer (`app-composer`; on the Mac
// a popover under the picked row; with the row scrolled above the page,
// the popover kept at the page's top, `app-composer-scrolled`) and a
// composer the page opened (`app-composer-page`). In wide windows, Android's and the Mac's, the
// threads pane beside the page (`app-threads-pane`), and with a pick's
// composer too (`app-threads-composer`: heading the pane on Android, a
// popover over the page on the Mac). An app in a pane on wide Android,
// at medium (720 dp), expanded (1024 dp) and large (`wide`, 1280 dp):
// under its top app bar (`app-pane`), a change under way in the bar
// (`app-pane-working`, also at 200%), with the threads side sheet over the
// page below large and beside it at large (`app-pane-threads`), a change
// under way opened in its side sheet (`app-pane-change`), the New version
// toast inline under the bar (`app-pane-toast`), and a failed change's
// chip between the bar and the page (`app-pane-failed`). The status and gesture bar stand-ins draw
// their icons in the brightness the screen's bands ask the system for
// (`SystemChrome.latestStyle`), not the theme's.
import 'dart:async';

import 'package:clax_app/agent/plan.dart';
import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/daemon/clock.dart';
import 'package:clax_app/platform/app_notice.dart';
import 'package:clax_app/platform/device/device_grants.dart';
import 'package:clax_app/platform/device/device_host.dart';
import 'package:clax_app/platform/device/reminders.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/build_plan.dart';
import 'package:clax_app/ui/bundle_sheets.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../platform/device/device_host_test.dart'
    show
        FakeCamera,
        FakeConsent,
        FakeFiles,
        FakeLocation,
        FakeNotifier,
        FakeShare;
import '../../support/fake_app_notice.dart';
import '../../support/fake_surface.dart';
import '../../support/fake_voice.dart';
import 'rig.dart';

/// A clock whose delays end only when [lapse] ends them: a toast stays
/// and the pill stays open until a shot lapses it.
class _Clock implements Clock {
  final _delays = <Completer<void>>[];
  @override
  DateTime now() => DateTime.utc(2026, 10, 19, 9, 41);
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

/// A build that runs, as the worker reports it.
class _Builds implements BuildControl {
  final _c = StreamController<BuildState>.broadcast(sync: true);
  final states = <String, BuildState>{};

  void emit(BuildState s) {
    states[s.appId] = s;
    _c.add(s);
  }

  @override
  Stream<BuildState> get builds => _c.stream;
  @override
  BuildState? buildOf(String appId) => states[appId];
  @override
  Future<void> stopBuild(String appId) async {}
  @override
  Future<void> resumeBuild(String appId) async {}
}

const _blue = Color(0xFF102A4C);

/// The text scales the pill's renders are taken at.
const _both = [1.0, 2.0];

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
  List<HostMessage> then = const [],
  int version = 1,
  _Clock? clock,
  FakeAppNotice? notice,
  bool hint = false,
  BuildLink? build,
  DeviceHost? device,
  Future<void> Function(String aid)? before,
  FutureOr<void> Function()? after,
  Stream<AppAction>? actions,
  bool bands = true,
  Stream<WorkerEvent> worker = const Stream.empty(),
  Future<void> Function(BuildContext, BundleFiles)? share,
}) async {
    final api = FakeClaxApi();
    final aid = api.seedArtifact('Runs', {'index.html': '<h1>Runs</h1>'});
    final style = ValueNotifier<SystemUiOverlayStyle?>(null);
    addTearDown(style.dispose);
    await before?.call(aid);
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
              worker: worker,
              share: share,
              clock: clock ?? _Clock(),
              notice: notice == null ? null : AppNoticeSlot(notice),
              firstRunHint: hint,
              build: build,
              device: device,
              actions: actions,
            ),
            IgnorePointer(child: _Bars(style, s.form)),
          ],
        ),
      ),
    );
    surface.push(
      Ready(
        artifact: 'x',
        version: version,
        latest: version,
        file: 'index.html',
      ),
    );
    surface.push(chrome);
    then.forEach(surface.push);
    await s.settle();
    if (after != null) {
      await after();
      await s.settle();
    }
    style.value = SystemChrome.latestStyle;
    await s.tester.pump();
    if (bands) {
      expect(style.value!.statusBarIconBrightness, status);
      expect(style.value!.systemNavigationBarIconBrightness, navigation);
    }
    await s.take(surfaceName);
    // Past a change chip's delayed open, so nothing is left waiting.
    await s.tester.pumpWidget(const SizedBox());
    await s.tester.pump(const Duration(seconds: 5));
}

/// A pick of the page's Wednesday row, where [_PageSurface] draws it.
const _wed = Draft(
  key: 'p1',
  label: '“Wed 8.0 km”',
  quote: 'Wed  8.0 km',
  text: '',
  clip: DraftClip.pending,
  at: Rect.fromLTWH(20, 110, 130, 32),
);

const _paneThreads = [
  ThreadSummary(
    id: 't1',
    status: 'open',
    summary: 'Show the weekly total under the list',
    sent: true,
    comments: 2,
    version: 1,
    label: '“Sat 12.4 km”',
  ),
  ThreadSummary(
    id: 't2',
    status: 'open',
    summary: 'Kilometres with one decimal, please',
    sent: false,
    comments: 1,
    version: 1,
    label: 'Whole app',
  ),
];

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
  shots(
    'app-working',
    (s) => _open(
      s,
      'app-working',
      _PageSurface(),
      const ChromeMsg(_blue, Colors.white),
      status: Brightness.light,
      navigation: Brightness.dark,
      then: const [Sent('thread-1')],
    ),
    forms: const [phone],
    textScales: _both,
  );
  Future<void> page(
    Shot s,
    String name, {
    List<HostMessage> then = const [],
    int version = 1,
    _Clock? clock,
    FakeAppNotice? notice,
    bool hint = false,
    BuildLink? build,
    DeviceHost? device,
    Future<void> Function(String aid)? before,
    FutureOr<void> Function()? after,
  }) => _open(
    s,
    name,
    _PageSurface(),
    const ChromeMsg(_blue, Colors.white),
    status: Brightness.light,
    navigation: Brightness.dark,
    then: then,
    version: version,
    clock: clock,
    notice: notice,
    hint: hint,
    build: build,
    device: device,
    before: before,
    after: after,
  );
  shots(
    'app-working-folded',
    (s) {
      final clock = _Clock();
      return page(
        s,
        'app-working-folded',
        clock: clock,
        then: const [Sent('thread-1')],
        after: clock.lapse,
      );
    },
    forms: const [phone],
  );
  shots('app-building', (s) {
    final plans = PlanStore();
    final builds = _Builds();
    return page(
      s,
      'app-building',
      build: BuildLink(plans: plans, control: builds),
      before: (aid) async {
        await plans.set(aid, [
          for (final (i, t) in const [
            'Log a run',
            'Weekly total',
            'Weekly chart',
            'Export',
            'Dark mode',
          ].indexed)
            PlanFeature(
              id: 'f$i',
              title: t,
              status: i < 2 ? FeatureStatus.done : FeatureStatus.todo,
            ),
        ]);
        builds.emit(BuildState(aid, BuildPhase.running, featureId: 'f2'));
      },
    );
  }, forms: const [phone], textScales: _both);
  shots(
    'app-new-version',
    (s) {
      final clock = _Clock();
      return page(
        s,
        'app-new-version',
        clock: clock,
        then: const [NewerMsg(2)],
        // The toast goes; the pill says it.
        after: clock.lapse,
      );
    },
    forms: const [phone],
    textScales: _both,
  );
  shots(
    'app-toast',
    (s) => page(s, 'app-toast', then: const [NewerMsg(2)]),
    forms: const [phone],
    textScales: _both,
  );
  shots('app-location', (s) {
    final location = FakeLocation();
    return page(
      s,
      'app-location',
      device: DeviceHost(
        grants: MemoryDeviceGrants(),
        consent: FakeConsent(),
        reminders: Reminders(FakeNotifier()),
        share: FakeShare(),
        files: FakeFiles(),
        camera: FakeCamera(),
        location: location,
      ),
      then: const [
        DeviceRequest(
          id: 'req-w',
          cap: 'location',
          method: 'watch',
          args: [
            {'watch': 'w'},
          ],
        ),
      ],
    );
  }, forms: const [phone], textScales: _both);
  shots(
    'app-hint',
    (s) => page(
      s,
      'app-hint',
      notice: FakeAppNotice(allow: false),
      hint: true,
    ),
    forms: const [phone],
    textScales: _both,
  );
  // A back-gesture zone as Android's default sensitivity gives it.
  void gestureZone(Shot s) => s.tester.view.systemGestureInsets =
      FakeViewPadding(right: 24 * s.form.dpr);
  shots(
    'app-comment-tab',
    (s) => page(
      s,
      'app-comment-tab',
      notice: FakeAppNotice(allow: false),
      before: (_) async => gestureZone(s),
    ),
    forms: const [phone],
    textScales: _both,
  );
  shots(
    'app-comment-tab-menu',
    (s) => page(
      s,
      'app-comment-tab-menu',
      notice: FakeAppNotice(allow: false),
      then: const [Sent('thread-1')],
      before: (_) async => gestureZone(s),
      after: () => s.tester.tap(find.byKey(const ValueKey('comment-tab'))),
    ),
    forms: const [phone],
  );
  shots(
    'app-rally',
    (s) => page(s, 'app-rally', version: 9, then: const [Landed(10)]),
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
  shots(
    'app-comment-tab-dark-page',
    (s) => _open(
      s,
      'app-comment-tab-dark-page',
      _PageSurface(
        head: black,
        body: const Color(0xFF121212),
        ink: const Color(0xFFE6E6E6),
        tabs: charcoal,
      ),
      const ChromeMsg(black, charcoal),
      status: Brightness.light,
      navigation: Brightness.light,
      notice: FakeAppNotice(allow: false),
      before: (_) async => gestureZone(s),
    ),
    forms: const [phone],
  );
  // Comment mode: the app's Comment action, then what the shell says.
  Future<void> commenting(
    Shot s,
    String name, {
    List<HostMessage> then = const [],
  }) async {
    final actions = StreamController<AppAction>.broadcast(sync: true);
    addTearDown(actions.close);
    final surface = _PageSurface();
    await _open(
      s,
      name,
      surface,
      const ChromeMsg(_blue, Colors.white),
      status: Brightness.light,
      navigation: Brightness.dark,
      actions: actions.stream,
      bands: false,
      after: () async {
        actions.add(AppAction.comment);
        await s.settle();
        then.forEach(surface.push);
      },
    );
  }

  shots(
    'app-comment-bar',
    (s) => commenting(
      s,
      'app-comment-bar',
      then: const [StateMsg(commenting: true, draft: null, busy: false)],
    ),
    forms: const [phone, mac],
    textScales: _both,
  );
  shots('app-comment-bar-narrow', (s) async {
    s.tester.view.physicalSize = Size(340 * s.form.dpr, 915 * s.form.dpr);
    await commenting(
      s,
      'app-comment-bar-narrow',
      then: const [StateMsg(commenting: true, draft: null, busy: false)],
    );
  }, forms: const [phone]);
  shots(
    'app-comment-waiting',
    (s) => commenting(
      s,
      'app-comment-waiting',
      then: const [
        StateMsg(commenting: true, draft: null, busy: false),
        PageWaitMsg(waiting: true),
      ],
    ),
    forms: const [phone, mac],
  );
  shots(
    'app-composer',
    (s) => commenting(
      s,
      'app-composer',
      then: const [
        StateMsg(
          commenting: false,
          draft: _wed,
          busy: false,
        ),
      ],
    ),
    forms: const [phone, mac],
  );
  shots(
    'app-composer-scrolled',
    (s) => commenting(
      s,
      'app-composer-scrolled',
      then: [
        StateMsg(
          commenting: false,
          draft: Draft(
            key: _wed.key,
            label: _wed.label,
            quote: _wed.quote,
            text: '',
            clip: DraftClip.pending,
            at: _wed.at!.translate(0, -600),
          ),
          busy: false,
        ),
      ],
    ),
    forms: const [mac],
  );
  // The threads pane: the app's Threads action, then what the shell says.
  Future<void> threadsOpen(
    Shot s,
    String name, {
    List<HostMessage> then = const [],
  }) async {
    final actions = StreamController<AppAction>.broadcast(sync: true);
    addTearDown(actions.close);
    final surface = _PageSurface();
    await _open(
      s,
      name,
      surface,
      const ChromeMsg(_blue, Colors.white),
      status: Brightness.light,
      navigation: Brightness.dark,
      actions: actions.stream,
      bands: false,
      after: () async {
        surface.push(const ThreadsMsg(_paneThreads));
        actions.add(AppAction.threads);
        await s.settle();
        then.forEach(surface.push);
      },
    );
  }

  shots(
    'app-threads-pane',
    (s) => threadsOpen(s, 'app-threads-pane'),
    forms: const [wide, mac],
  );
  shots(
    'app-threads-composer',
    (s) => threadsOpen(
      s,
      'app-threads-composer',
      then: const [StateMsg(commenting: false, draft: _wed, busy: false)],
    ),
    forms: const [wide, mac],
  );
  shots(
    'app-composer-page',
    (s) => commenting(
      s,
      'app-composer-page',
      then: const [
        StateMsg(
          commenting: false,
          draft: Draft(
            key: 'g1',
            kind: 'custom',
            label: 'How did this run feel?',
            text: '',
            clip: DraftClip.none,
            byPage: true,
          ),
          busy: false,
        ),
      ],
    ),
    forms: const [phone, mac],
  );

  // An app in a pane on wide Android.
  const medium = ShotForm(
    'medium',
    Size(720, 1024),
    2,
    TargetPlatform.android,
    top: 24,
    bottom: 16,
  );
  const expanded = ShotForm(
    'expanded',
    Size(1024, 768),
    2,
    TargetPlatform.android,
    top: 24,
    bottom: 16,
  );
  const panes = [medium, expanded, wide];
  Future<void> inPane(
    Shot s,
    String name, {
    List<HostMessage> then = const [],
    FutureOr<void> Function(StreamController<WorkerEvent> worker, String aid)?
    after,
    _Clock? clock,
  }) async {
    var aid = '';
    final worker = StreamController<WorkerEvent>.broadcast(sync: true);
    addTearDown(worker.close);
    final surface = _PageSurface();
    await _open(
      s,
      name,
      surface,
      const ChromeMsg(_blue, Colors.white),
      status: Brightness.light,
      navigation: Brightness.dark,
      bands: false,
      clock: clock,
      worker: worker.stream,
      share: (_, _) async {},
      then: [const ThreadsMsg(_paneThreads), ...then],
      before: (a) async => aid = a,
      after: after == null ? null : () => after(worker, aid),
    );
  }

  shots('app-pane', (s) => inPane(s, 'app-pane'), forms: panes);
  shots(
    'app-pane-threads',
    (s) => inPane(
      s,
      'app-pane-threads',
      after: (_, _) => s.tester.tap(find.byKey(const ValueKey('pane-threads'))),
    ),
    forms: panes,
  );
  shots(
    'app-pane-working',
    (s) => inPane(s, 'app-pane-working', then: const [Sent('thread-1')]),
    forms: panes,
    textScales: _both,
  );
  shots(
    'app-pane-change',
    (s) => inPane(
      s,
      'app-pane-change',
      then: const [Sent('thread-1')],
      after: (_, _) => s.tester.tap(find.byKey(const ValueKey('pane-status'))),
    ),
    forms: panes,
  );
  shots(
    'app-pane-toast',
    (s) => inPane(s, 'app-pane-toast', then: const [NewerMsg(2)]),
    forms: panes,
  );
  shots(
    'app-pane-failed',
    (s) => inPane(
      s,
      'app-pane-failed',
      then: const [Sent('thread-1')],
      after: (worker, aid) => worker.add(
        WorkerFailed(aid, 'Clax couldn’t finish this change.'),
      ),
    ),
    forms: panes,
  );
}
