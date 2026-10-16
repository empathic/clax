import 'dart:async';

import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/home/home_host.dart';
import 'package:clax_app/home/home_route.dart';
import 'package:clax_app/home/home_settings.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/make_service.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_surface.dart';
import '../support/fake_voice.dart';

const _page = {'index.html': '<h1>x</h1>'};

class _Actions implements HomeActions {
  final menus = <(String, Rect)>[];
  @override
  void open(String id) {}
  @override
  void openMake(String id) {}
  @override
  void newApp() {}
  @override
  void menu(String id, Rect anchor) => menus.add((id, anchor));
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
    VoidCallback? ready;

    Future<void> pump(WidgetTester tester) => tester.pumpWidget(
      MaterialApp(
        home: HomeRoute(
          host: host,
          app: (context, aid, onReady) {
            ready = onReady;
            return _Page(aid);
          },
          fallback: (_) => const Text('built-in'),
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
        'over only at its first ready, its page kept', (tester) async {
      await pump(tester);
      expect(find.text('built-in'), findsOneWidget);

      settings.homeAppId = home;
      host.settingsChanged();
      await tester.pump();
      // Loading under the built-in screen: nothing empty shows.
      expect(find.text('page $home'), findsOneWidget);
      expect(find.text('built-in'), findsOneWidget);
      final stack = tester.widget<Stack>(
        find.descendant(
          of: find.byType(HomeRoute),
          matching: find.byType(Stack),
        ),
      );
      expect(
        (stack.children.last.key as ValueKey).value,
        'home-fallback',
        reason: 'the built-in screen is on top',
      );

      ready!();
      await tester.pump();
      expect(find.text('built-in'), findsNothing);
      expect(find.text('page $home'), findsOneWidget);
      expect(_Page.made, 1, reason: 'the page loaded under the cover is kept');
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
              app: (context, aid, onReady) => AppScreen(
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
                onReady: (_) => onReady(),
              ),
              fallback: (_) => const Text('built-in'),
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
}
