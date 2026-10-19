import 'dart:async';
import 'dart:io';

import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/bundle/installer.dart';
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/daemon/clock.dart';
import 'package:clax_app/home/home_install.dart';
import 'package:clax_app/home/home_recovery.dart';
import 'package:clax_app/home/home_host.dart';
import 'package:clax_app/home/home_route.dart';
import 'package:clax_app/home/home_settings.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_menu.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/launcher.dart';
import 'package:clax_app/ui/make_service.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_surface.dart';
import '../support/fake_voice.dart';

const _page = {'index.html': '<h1>x</h1>'};

/// A clock whose delays complete as the test advances it.
class _Clock implements Clock {
  var _t = Duration.zero;
  final _waits = <(Duration, Completer<void>)>[];

  @override
  DateTime now() => DateTime.utc(2026, 10, 16).add(_t);

  @override
  Future<void> delay(Duration d) {
    final c = Completer<void>();
    _waits.add((_t + d, c));
    return c.future;
  }

  void advance(Duration d) {
    _t += d;
    for (final w in _waits.where((w) => w.$1 <= _t).toList()) {
      _waits.remove(w);
      w.$2.complete();
    }
  }
}

class _Actions implements HomeActions {
  final menus = <(String, Rect)>[];
  @override
  void open(String id) {}
  @override
  void openMake(String id) {}
  @override
  void newApp() {}
  @override
  Future<String?> menu(
    String id,
    Rect anchor,
    List<HomeMenuItem> items,
  ) async {
    menus.add((id, anchor));
    return null;
  }
  @override
  void openInbox() {}
  @override
  void openSettings() {}
  @override
  void importApp() {}
}

/// Counts its states, so a test sees whether the home app's page was made
/// again.
class _Page extends StatefulWidget {
  const _Page(this.aid);
  final String aid;
  static var made = 0;
  @override
  State<_Page> createState() => _PageState();
}

class _PageState extends State<_Page> {
  @override
  void initState() {
    super.initState();
    _Page.made++;
  }

  @override
  Widget build(BuildContext context) =>
      Text('page ${widget.aid}', textDirection: TextDirection.ltr);
}

void main() {
  late FakeClaxApi api;
  late String home;
  late String other;
  late HomeHost host;
  late HomeSettings settings;
  late _Actions actions;

  setUp(() {
    var t = 0;
    api = FakeClaxApi(
      clock: () => '2026-10-12T00:00:${(t++).toString().padLeft(2, '0')}Z',
    );
    home = api.seedArtifact('Home', _page, capabilities: {'home': {}});
    other = api.seedArtifact('Notes', _page);
    actions = _Actions();
    settings = HomeSettings();
    host = HomeHost(
      api: api,
      stream: api.stream(),
      closeStream: true,
      makes: MakeService((r, c, s) => Completer<String>().future),
      builds: null,
      settings: settings,
      actions: actions,
    );
    _Page.made = 0;
  });

  tearDown(() => host.dispose());

  group('which home screen shows', () {
    Future<void> pump(WidgetTester tester) => tester.pumpWidget(
      MaterialApp(
        home: HomeRoute(
          host: host,
          clock: _Clock(),
          app: (context, aid, hooks) => _Page(aid),
          fallback: (_, _) => const Text('built-in'),
        ),
      ),
    );

    testWidgets('no home app: the built-in home screen', (tester) async {
      await pump(tester);
      expect(find.text('built-in'), findsOneWidget);
      expect(find.byType(_Page), findsNothing);
    });

    testWidgets('a recorded home app shows at once, alone', (tester) async {
      settings.homeAppId = home;
      await pump(tester);
      expect(find.text('page $home'), findsOneWidget);
      expect(find.text('built-in'), findsNothing);
    });

    testWidgets('chosen built-in: the built-in screen, home app or not', (
      tester,
    ) async {
      settings
        ..homeAppId = home
        ..builtin = true;
      await pump(tester);
      expect(find.text('built-in'), findsOneWidget);
      expect(find.byType(_Page), findsNothing);
    });

    testWidgets('a home app installed while the built-in screen shows takes '
        'over only once its page says the grid is drawn: not at the '
        "frame's ready, nor at its apps() read", (tester) async {
      final surface = FakeAppSurface();
      await tester.pumpWidget(
        MaterialApp(
          home: HomeRoute(
            host: host,
            clock: _Clock(),
            app: (context, aid, hooks) {
              return AppScreen(
                aid: aid,
                base: Uri.parse('http://127.0.0.1:7999/'),
                api: api,
                surface: surface,
                voice: FakeVoice(),
                settings: AppSettings(),
                timings: const NoTimings(),
                worker: const Stream.empty(),
                home: host,
                mode: AppMode.home,
              );
            },
            fallback: (_, _) => const Text('built-in'),
          ),
        ),
      );
      expect(find.text('built-in'), findsOneWidget);

      settings.homeAppId = home;
      host.settingsChanged();
      await tester.pump();
      expect(find.byType(AppScreen), findsOneWidget);
      expect(find.text('built-in'), findsOneWidget);
      final page = tester.state(find.byType(AppScreen));
      Object? topKey() =>
          (tester
                      .widget<Stack>(
                        find
                            .descendant(
                              of: find.byType(HomeRoute),
                              matching: find.byType(Stack),
                            )
                            .first,
                      )
                      .children
                      .last
                      .key
                  as ValueKey)
              .value;
      expect(topKey(), 'home-fallback', reason: 'the built-in screen on top');

      // The page's real order: the frame loads, then the page asks for
      // the capability and its apps, then draws them and says so.
      surface.push(
        Ready(artifact: home, version: 1, latest: 1, file: 'index.html'),
      );
      await tester.pump();
      expect(find.text('built-in'), findsOneWidget, reason: 'ready alone');
      for (final (id, method) in [('1', 'available'), ('2', 'apps')]) {
        surface.push(
          DeviceRequest(id: id, cap: 'home', method: method, args: const []),
        );
        await tester.pump();
        await tester.pump();
      }
      final answers = surface.hostCalls.whereType<DeviceResult>().toList();
      expect(answers.map((a) => a.id), ['1', '2']);
      expect(find.text('built-in'), findsOneWidget, reason: 'apps read alone');

      surface.push(
        const DeviceRequest(id: '3', cap: 'home', method: 'shown', args: []),
      );
      await tester.pump();
      await tester.pump();
      expect(find.text('built-in'), findsNothing);
      expect(find.byType(AppScreen), findsOneWidget);
      expect(
        tester.state(find.byType(AppScreen)),
        same(page),
        reason: 'the page loaded under the cover is kept',
      );
    });

    testWidgets('the home app under the built-in screen acts as covered: '
        'no menu bar commands, focus, Escape or back, and its events are '
        'held until it shows', (tester) async {
      final surface = FakeAppSurface();
      final cmd = CommentsCommand();
      addTearDown(cmd.dispose);
      await tester.pumpWidget(
        MaterialApp(
          home: HomeRoute(
            host: host,
            clock: _Clock(),
            app: (context, aid, hooks) => AppScreen(
              aid: aid,
              base: Uri.parse('http://127.0.0.1:7999/'),
              api: api,
              surface: surface,
              voice: FakeVoice(),
              settings: AppSettings(),
              timings: const NoTimings(),
              worker: const Stream.empty(),
              home: host,
              mode: AppMode.home,
              comments: cmd,
              covered: hooks.covered,
            ),
            fallback: (_, _) => const Text('built-in'),
          ),
        ),
      );
      settings.homeAppId = home;
      host.settingsChanged();
      await tester.pump();
      await tester.pump();
      expect(find.text('built-in'), findsOneWidget);
      expect(surface.visibility, [false], reason: 'told hidden, never shown');
      expect(cmd.toggle, isNull, reason: 'no Comment or Threads');
      expect(
        FocusManager.instance.primaryFocus?.debugLabel,
        isNot('app screen'),
      );
      final nav = tester.state<NavigatorState>(find.byType(Navigator));
      expect(await nav.maybePop(), isFalse, reason: 'back is not taken');

      // The page reads, then a change arrives while it is covered.
      surface.push(
        const DeviceRequest(id: 'u', cap: 'home', method: 'unread', args: []),
      );
      await tester.pump();
      api.pushInbox('reply', artifactId: other, body: 'one');
      await tester.pump();
      await tester.pump();
      Iterable<HomeDeviceEvent> told() =>
          surface.hostCalls.whereType<HomeDeviceEvent>();
      expect(told(), isEmpty, reason: 'held while covered');

      surface.push(
        const DeviceRequest(id: 's', cap: 'home', method: 'shown', args: []),
      );
      await tester.pump();
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 500));
      expect(find.text('built-in'), findsNothing);
      expect(surface.visibility.last, isTrue);
      expect(cmd.toggle, isNotNull);
      expect(told().map((e) => (e.name, e.data)), contains(('unread', 1)));
      expect(FocusManager.instance.primaryFocus?.debugLabel, 'app screen');
    });

    testWidgets('a shown from another app lifts nothing', (tester) async {
      await pump(tester);
      settings.homeAppId = home;
      host.settingsChanged();
      await tester.pump();
      await expectLater(
        host.call(other, 'shown', const {}),
        throwsA(isA<HomeRefused>()),
      );
      await tester.pump();
      expect(find.text('built-in'), findsOneWidget);
    });
  });

  group('the home app screen in home mode', () {
    Future<FakeAppSurface> pump(
      WidgetTester tester, {
      EdgeInsets padding = EdgeInsets.zero,
    }) async {
      settings.homeAppId = home;
      final surface = FakeAppSurface();
      await tester.pumpWidget(
        MaterialApp(
          home: Padding(
            padding: padding,
            child: HomeRoute(
              host: host,
              clock: _Clock(),
              app: (context, aid, hooks) => AppScreen(
                aid: aid,
                base: Uri.parse('http://127.0.0.1:7999/'),
                api: api,
                surface: surface,
                voice: FakeVoice(),
                settings: AppSettings(),
                timings: const NoTimings(),
                worker: const Stream.empty(),
                home: host,
                mode: AppMode.home,
              ),
              fallback: (_, _) => const Text('built-in'),
            ),
          ),
        ),
      );
      await tester.pump();
      return surface;
    }

    Iterable<(String, Object?)> told(FakeAppSurface s) =>
        s.hostCalls.whereType<HomeDeviceEvent>().map((e) => (e.name, e.data));

    testWidgets('back at its root leaves Clax: nothing pops to a launcher', (
      tester,
    ) async {
      await pump(tester);
      final nav = tester.state<NavigatorState>(find.byType(Navigator));
      expect(nav.canPop(), isFalse);
      final handled = await tester.binding.handlePopRoute();
      await tester.pump();
      expect(handled, isFalse, reason: 'the system takes back: Clax closes');
      expect(find.byType(AppScreen), findsOneWidget);
      expect(find.text('built-in'), findsNothing);
    });

    testWidgets('changes while covered are told on return, the latest of each '
        'topic', (tester) async {
      final surface = await pump(tester);
      // The page reads first; the host then pushes its changes.
      surface.push(
        const DeviceRequest(id: 'a', cap: 'home', method: 'unread', args: []),
      );
      await tester.pump();
      final nav = tester.state<NavigatorState>(find.byType(Navigator));
      unawaited(
        nav.push(MaterialPageRoute<void>(builder: (_) => const Text('inbox'))),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 500));

      api.pushInbox('reply', artifactId: other, body: 'one');
      api.pushInbox('reply', artifactId: other, body: 'two');
      await tester.pump();
      await tester.pump();
      expect(told(surface), isEmpty, reason: 'nothing told while covered');

      nav.pop();
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 500));
      expect(told(surface), contains(('unread', 2)));
      expect(
        told(surface).where((e) => e.$1 == 'unread'),
        hasLength(1),
        reason: 'only the latest',
      );
    });

    testWidgets('a menu rect from the page is moved onto the screen', (
      tester,
    ) async {
      final surface = await pump(
        tester,
        padding: const EdgeInsets.only(top: 40, left: 10),
      );
      await tester.pump();
      surface.push(
        DeviceRequest(
          id: 'm',
          cap: 'home',
          method: 'menu',
          args: [
            {
              'id': other,
              'rect': {'x': 5, 'y': 100, 'w': 60, 'h': 70},
            },
          ],
        ),
      );
      await tester.pump();
      await tester.pump();
      final origin = tester.getTopLeft(
        find.byKey(const ValueKey('fake-surface')),
      );
      expect(origin, const Offset(10, 40));
      expect(actions.menus, [(other, Rect.fromLTWH(15, 140, 60, 70))]);
    });
  });
  group('the fallback when the home app fails', () {
    late Directory tmp;
    late _Clock clock;
    late HomeRecovery recovery;
    HomePageHooks? hooks;

    setUp(() {
      tmp = Directory.systemTemp.createTempSync('home-route');
      clock = _Clock();
      recovery = HomeRecovery(
        api: api,
        installer: Installer(
          api: api,
          store: LineageStore(Directory('${tmp.path}/support')),
          key: homeInstallKey(),
        ),
        settings: settings,
      );
      hooks = null;
    });
    tearDown(() => tmp.deleteSync(recursive: true));

    Future<void> pump(WidgetTester tester) async {
      settings.homeAppId = home;
      await tester.pumpWidget(
        MaterialApp(
          home: HomeRoute(
            host: host,
            recovery: recovery,
            clock: clock,
            app: (context, aid, h) {
              hooks = h;
              return _Page(aid);
            },
            fallback: (_, banner) => Launcher(
              api: api,
              open: (_) {},
              make: (r, c, s) => Completer<String>().future,
              voice: FakeVoice.unsupported('No speech here'),
              host: host,
              banner: banner,
            ),
          ),
        ),
      );
      await tester.pump();
    }

    final banner = find.text('Your home screen didn’t load');

    testWidgets('no shown within 10 s: the built-in screen with the banner', (
      tester,
    ) async {
      await pump(tester);
      expect(find.byType(Launcher), findsNothing);
      clock.advance(const Duration(milliseconds: 9999));
      await tester.pump();
      expect(find.byType(Launcher), findsNothing);
      clock.advance(const Duration(milliseconds: 1));
      await tester.pump();
      await tester.pump();
      expect(find.byType(Launcher), findsOneWidget);
      expect(banner, findsOneWidget);
      expect(find.text('Undo last change'), findsOneWidget);
      expect(find.text('Reset'), findsOneWidget);
    });

    testWidgets('shown at 9.9 s: no fallback, then or later', (tester) async {
      await pump(tester);
      clock.advance(const Duration(milliseconds: 9900));
      await host.call(home, 'shown', const {});
      await tester.pump();
      clock.advance(const Duration(seconds: 30));
      await tester.pump();
      expect(find.byType(Launcher), findsNothing);
      expect(find.text('page $home'), findsOneWidget);
    });

    testWidgets('a late shown takes the fallback away again', (tester) async {
      await pump(tester);
      clock.advance(const Duration(seconds: 10));
      await tester.pump();
      await tester.pump();
      expect(banner, findsOneWidget);
      await host.call(home, 'shown', const {});
      await tester.pump();
      expect(find.byType(Launcher), findsNothing);
    });

    testWidgets('"This app stopped loading": the fallback with the banner', (
      tester,
    ) async {
      await pump(tester);
      await host.call(home, 'shown', const {});
      await tester.pump();
      hooks!.onStoppedLoading();
      await tester.pump();
      expect(banner, findsOneWidget);
    });

    testWidgets('Undo restores the version before the latest, and the home '
        'app shows again once its page is shown', (tester) async {
      await api.registerAppSession(installId: 'i', pid: 1);
      api.seedVersion(home, {'index.html': '<p>broken</p>'});
      await pump(tester);
      hooks!.onStoppedLoading();
      await tester.pump();
      expect(_Page.made, 1);

      await tester.tap(find.text('Undo last change'));
      await tester.pump();
      await tester.pump();
      final restore = api.calls.lastWhere((c) => c.name == 'restoreVersion');
      expect(restore.args, {'aid': home, 'n': 1, 'ifVersion': 2});
      expect(_Page.made, 2, reason: 'the page loads again');
      // Loading under the built-in screen, without the banner.
      expect(find.byType(Launcher), findsOneWidget);
      expect(banner, findsNothing);

      await host.call(home, 'shown', const {});
      await tester.pump();
      expect(find.byType(Launcher), findsNothing);
      expect(find.text('page $home'), findsOneWidget);
    });

    testWidgets('a failed Undo says why on the banner', (tester) async {
      await pump(tester);
      hooks!.onStoppedLoading();
      await tester.pump();
      await tester.tap(find.text('Undo last change'));
      await tester.pump();
      await tester.pump();
      expect(
        find.text('Your home screen has no earlier version.'),
        findsOneWidget,
      );
      expect(banner, findsOneWidget);
    });

    testWidgets('the built-in home screen, chosen, shows without the banner '
        'and survives a restart', (tester) async {
      final dir = Directory('${tmp.path}/support');
      final again = (await tester.runAsync(() async {
        final saved = await HomeSettings.load(dir);
        saved
          ..homeAppId = home
          ..builtin = true;
        await saved.save();
        return HomeSettings.load(dir);
      }))!;
      expect(again.builtin, isTrue);
      settings.builtin = again.builtin;
      await pump(tester);
      clock.advance(const Duration(seconds: 10));
      await tester.pump();
      expect(find.byType(Launcher), findsOneWidget);
      expect(find.byType(_Page), findsNothing);
      expect(banner, findsNothing);
    });
  });
}
