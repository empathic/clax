// Renders of the split layout: the sidebar beside the home grid (Home
// selected) and beside an open app, with two starred apps, a group and a
// make, light and dark, as `split-home-<form>-<theme>.png` and
// `split-app-<form>-<theme>.png`; and on the Mac, `split-hover-mac-<theme>`:
// the app's row selected while the sidebar has the keyboard, in the
// system's accent (macOS's default blue), with the mouse over another row;
// and `split-make-<form>-<theme>`: the make selected, its build view in the
// detail with the page streaming into the preview.
//
// The Mac form is drawn under the main window's real toolbar row (52
// points, the window buttons' corner 79 points across, as
// macos/Runner/WindowBridge.swift measures it on macOS 26), with stand-ins
// for what flutter_test cannot draw: the sidebar material behind the clear
// sidebar, the window buttons, and the native toolbar's items. The wide
// form is an Android tablet's, with the Material list pane.
//
//   SHOTS="$PWD/target/flutter-app/shots"; mkdir -p "$SHOTS"
//   bazelisk test //apps/clax_app:ui_shots_split_shots_test \
//     --nocache_test_results --test_env=CLAX_SHOTS_DIR="$SHOTS" \
//     --sandbox_writable_path="$SHOTS"
import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/ui/kit/kit.dart';
import 'package:clax_app/ui/making_preview.dart';
import 'package:clax_app/ui/making_screen.dart';
import 'package:clax_app/ui/split/sidebar_model.dart';
import 'package:clax_app/ui/split/split_shell.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/tokens.g.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../support/split_fixture.dart';
import 'rig.dart';

const _bar = WindowTitlebar(top: 52, leading: 79);

/// macOS's default accent (Multicolor), light and dark.
final accent = ValueNotifier<SystemAccent?>((
  light: const Color(0xFF007AFF),
  dark: const Color(0xFF0A84FF),
));

/// The sidebar's width in the Mac renders.
const double _sidebar = 220;

/// The home app's grid as its page draws it, in the app's colours.
class _Grid extends StatelessWidget {
  const _Grid();

  @override
  Widget build(BuildContext context) {
    final p = claxPalette(context);
    final platform = Theme.of(context).platform;
    const tiles = [
      ('Runs', Color(0xFF3B7DD8)),
      ('Notes', Color(0xFFE0A030)),
      ('Groceries', Color(0xFF2F8F5B)),
      ('Tools', null),
      ('Budget', Color(0xFF8A5CC2)),
      ('Weather', Color(0xFF2CA6B8)),
    ];
    return ColoredBox(
      color: p.bg,
      child: SafeArea(
        child: Padding(
          padding: const EdgeInsets.all(32),
          child: Wrap(
            spacing: 28,
            runSpacing: 24,
            children: [
              for (final (name, colour) in tiles)
                SizedBox(
                  width: 80,
                  child: Column(
                    children: [
                      Container(
                        width: 64,
                        height: 64,
                        decoration: BoxDecoration(
                          color: colour ?? p.fg.withValues(alpha: 0.08),
                          borderRadius: BorderRadius.circular(
                            ClaxRadius.tile(64),
                          ),
                        ),
                        child: colour == null
                            ? Icon(Icons.folder_outlined, color: p.muted)
                            : null,
                      ),
                      const SizedBox(height: 6),
                      Text(
                        name,
                        style: ClaxType.of(
                          ClaxRole.callout,
                          platform,
                        ).copyWith(color: p.fg),
                      ),
                    ],
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }
}

/// An app's page as its WebView would show it: a light page whatever the
/// theme.
class _Page extends StatelessWidget {
  const _Page();

  @override
  Widget build(BuildContext context) {
    const item = TextStyle(
      fontFamily: '.AppleSystemUIFont',
      fontSize: 17,
      color: Color(0xFF1D1D1F),
    );
    return const ColoredBox(
      color: Color(0xFFFFFFFF),
      child: SafeArea(
        child: Padding(
          padding: EdgeInsets.all(32),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                'Groceries',
                style: TextStyle(
                  fontFamily: '.AppleSystemUIFont',
                  fontSize: 34,
                  fontWeight: FontWeight.w700,
                  color: Color(0xFF1D1D1F),
                ),
              ),
              SizedBox(height: 16),
              Text('Oat milk', style: item),
              SizedBox(height: 8),
              Text('Lemons', style: item),
              SizedBox(height: 8),
              Text('Coffee beans', style: item),
            ],
          ),
        ),
      ),
    );
  }
}

Widget _detail(SidebarSelection s) => const Material(child: _Page());

/// A tide table as the make's live preview shows it.
class _Preview implements PreviewView {
  final _loaded = ValueNotifier(false);

  @override
  Future<void> loadHtml(String html) async => _loaded.value = true;

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: _loaded,
    builder: (context, loaded, _) => loaded
        ? ColoredBox(
            color: const Color(0xFFFFFFFF),
            child: Padding(
              padding: const EdgeInsets.all(28),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  const Text(
                    'Harbour tides',
                    style: TextStyle(
                      fontFamily: '.AppleSystemUIFont',
                      fontSize: 28,
                      fontWeight: FontWeight.w700,
                      color: Color(0xFF1D1D1F),
                    ),
                  ),
                  const SizedBox(height: 12),
                  for (final t in const [
                    '04:12  High  4.1 m',
                    '10:31  Low  0.6 m',
                    '16:40  High  4.3 m',
                  ])
                    Padding(
                      padding: const EdgeInsets.symmetric(vertical: 4),
                      child: Text(
                        t,
                        style: const TextStyle(
                          fontFamily: '.AppleSystemUIFont',
                          fontSize: 17,
                          color: Color(0xFF1D1D1F),
                        ),
                      ),
                    ),
                ],
              ),
            ),
          )
        : const SizedBox(),
  );

  @override
  void dispose() {}
}

/// The detail with the make's build view for a make, [f]'s make streaming
/// its page.
Widget Function(SidebarSelection) _makeDetail(SplitFixture f) =>
    (s) => switch (s) {
      MakeSelected(:final id) => MakingScreen(
        request: f.host.sessionOf(id)!.request,
        session: f.host.sessionOf(id),
        preview: _Preview.new,
        haikus: () async => const [],
        onExit: (_) {},
      ),
      _ => _detail(s),
    };

/// Selects [f]'s make in [model].
void _making(SplitFixture f, SidebarModel model) =>
    model.select(MakeSelected(f.host.makeIdOf(f.makes.makes.single)));

/// Streams the page into the selected make's preview.
Future<void> _stream(WidgetTester tester, SplitFixture f) async {
  f.report!(const MakeProgress(Narration('Laying out the day’s tides.')));
  f.report!(MakeProgress(toolStarted('write_file', {'path': 'index.html'})));
  f.report!(
    const MakeProgress(PartialFile('index.html', '<h1>Harbour tides</h1>')),
  );
  await tester.pump();
}

/// The window's sidebar material behind the clear sidebar, [width] wide.
class _Material extends StatelessWidget {
  const _Material({required this.width, required this.dark});
  final double width;
  final bool dark;

  @override
  Widget build(BuildContext context) => Align(
    alignment: Alignment.centerLeft,
    child: SizedBox(
      width: width,
      height: double.infinity,
      child: ColoredBox(
        color: dark ? const Color(0xFF2B2A2E) : const Color(0xFFE9E8EC),
      ),
    ),
  );
}

/// The native toolbar's items over the row: the sidebar's toggle after
/// the window buttons; past the sidebar's edge (the spacer
/// `SidebarEdgeView` in macos/Runner/Toolbar.swift), Back and Forward,
/// then the title; and Comment, Threads and (for an app) Share at the
/// trailing end.
class _Toolbar extends StatelessWidget {
  const _Toolbar({
    required this.title,
    this.subtitle,
    required this.dark,
    this.share = true,
  });
  final String title;
  final String? subtitle;
  final bool dark, share;

  @override
  Widget build(BuildContext context) {
    final ink = dark ? const Color(0xFFDDDDDD) : const Color(0xFF3C3C43);
    Widget icon(IconData i) => Padding(
      padding: const EdgeInsets.symmetric(horizontal: 9),
      child: Icon(i, size: 17, color: ink),
    );
    return IgnorePointer(
      child: SizedBox(
        height: _bar.top,
        child: Row(
          children: [
            SizedBox(width: _bar.leading + 8),
            icon(Icons.view_sidebar_outlined),
            // To the sidebar's edge, then the toolbar's item spacing.
            SizedBox(width: _sidebar - (_bar.leading + 8 + 35) + 8),
            icon(Icons.chevron_left),
            icon(Icons.chevron_right),
            const SizedBox(width: 10),
            Column(
              mainAxisAlignment: MainAxisAlignment.center,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  title,
                  style: TextStyle(
                    fontFamily: '.AppleSystemUIFont',
                    fontSize: 13,
                    fontWeight: FontWeight.w700,
                    color: dark ? Colors.white : Colors.black,
                  ),
                ),
                if (subtitle case final s?)
                  Text(
                    s,
                    style: TextStyle(
                      fontFamily: '.AppleSystemUIFont',
                      fontSize: 11,
                      color: ink,
                    ),
                  ),
              ],
            ),
            const Spacer(),
            icon(Icons.chat_bubble_outline),
            icon(Icons.view_list_outlined),
            if (share) icon(Icons.ios_share),
            const SizedBox(width: 10),
          ],
        ),
      ),
    );
  }
}

/// Close, minimise and zoom as macOS 26 draws them in the 52-point row.
class _WindowButtons extends CustomPainter {
  const _WindowButtons();

  @override
  void paint(Canvas canvas, Size size) {
    const colours = [Color(0xFFFF5F57), Color(0xFFFEBC2E), Color(0xFF28C840)];
    for (var i = 0; i < 3; i++) {
      canvas.drawCircle(
        Offset(_bar.leading - 7 - 23.0 * (2 - i), _bar.top / 2),
        7,
        Paint()..color = colours[i],
      );
    }
  }

  @override
  bool shouldRepaint(_WindowButtons old) => false;
}

void main() {
  final dir = shotsDir;

  Future<(SplitFixture, SidebarModel)> fixture(WidgetTester tester) async {
    final f = SplitFixture();
    await tester.runAsync(f.start);
    final model = SidebarModel(host: f.host, unreadOf: (id) => id == f.ids['Notes'] ? 2 : 0);
    for (var i = 0; i < 5; i++) {
      await tester.runAsync(() => Future<void>.delayed(Duration.zero));
    }
    return (f, model);
  }

  for (final scene in ['home', 'app', 'hover', 'make']) {
    final app = scene == 'app' || scene == 'hover';
    final make = scene == 'make';
    for (final dark in [false, true]) {
      final name = 'split-$scene-mac-${dark ? 'dark' : 'light'}';
      testWidgets(name, skip: dir == null, (tester) async {
        await tester.runAsync(loadFonts);
        final comparator = goldenFileComparator;
        final update = autoUpdateGoldenFiles;
        goldenFileComparator = LocalFileComparator(Uri.file('${dir!}/x.png'));
        autoUpdateGoldenFiles = true;
        tester.view.physicalSize = const Size(1200, 800) * 2;
        tester.view.devicePixelRatio = 2;
        addTearDown(tester.view.reset);
        debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
        debugDisableShadows = false;
        try {
          final (f, model) = await fixture(tester);
          if (app) {
            model.toggleGroup('tools');
            model.select(AppSelected(f.ids['Groceries']!));
          }
          if (make) _making(f, model);
          await tester.pumpWidget(
            Builder(
              builder: (context) {
                final data = MediaQuery.of(context);
                return WindowTitlebarScope(
                  titlebar: _bar,
                  child: MediaQuery(
                    data: data.copyWith(
                      padding: data.padding.copyWith(top: _bar.top),
                      viewPadding: data.viewPadding.copyWith(top: _bar.top),
                    ),
                    child: Directionality(
                      textDirection: TextDirection.ltr,
                      child: Stack(
                      key: const ValueKey('mac-window'),
                      children: [
                        _Material(width: 220, dark: dark),
                        SystemAccentScope(
                          notifier: accent,
                          child: MaterialApp(
                            debugShowCheckedModeBanner: false,
                            theme: claxTheme(
                              dark ? Brightness.dark : Brightness.light,
                              TargetPlatform.macOS,
                            ),
                            home: SplitShell(
                              model: model,
                              home: const Material(child: _Grid()),
                              detailFor: make ? _makeDetail(f) : _detail,
                            ),
                          ),
                        ),
                        _Toolbar(
                          title: app
                              ? 'Groceries'
                              : make
                              ? 'A tide table'
                              : 'Clax',
                          subtitle: app ? 'Version 4' : null,
                          dark: dark,
                          share: app,
                        ),
                        const IgnorePointer(
                          child: CustomPaint(
                            size: Size.infinite,
                            painter: _WindowButtons(),
                          ),
                        ),
                      ],
                    ),
                    ),
                  ),
                );
              },
            ),
          );
          for (var i = 0; i < 6; i++) {
            await tester.pump(const Duration(milliseconds: 100));
          }
          if (make) {
            await _stream(tester, f);
            for (var i = 0; i < 4; i++) {
              await tester.pump(const Duration(milliseconds: 100));
            }
          }
          if (scene == 'hover') {
            // The keyboard in the sidebar, and the mouse over Budget.
            await tester.tap(
              find.byKey(ValueKey('sidebar-app-${f.ids['Groceries']}')),
            );
            final mouse = await tester.createGesture(
              kind: PointerDeviceKind.mouse,
            );
            await mouse.addPointer(
              location: tester.getCenter(
                find.byKey(ValueKey('sidebar-app-${f.ids['Budget']}')),
              ),
            );
            addTearDown(mouse.removePointer);
            await tester.pump();
          }
          // Nothing of the sidebar under the window buttons.
          expect(
            tester.getTopLeft(find.text('Home')).dy,
            greaterThan(_bar.top),
          );
          await expectLater(
            find.byKey(const ValueKey('mac-window')),
            matchesGoldenFile('$name.png'),
          );
          model.dispose();
          f.dispose();
        } finally {
          goldenFileComparator = comparator;
          autoUpdateGoldenFiles = update;
          debugDisableShadows = true;
          debugDefaultTargetPlatformOverride = null;
        }
      });
    }
  }

  for (final scene in ['home', 'app', 'make']) {
    shots('split-$scene', (s) async {
      final (f, model) = await fixture(s.tester);
      if (scene == 'app') model.select(AppSelected(f.ids['Groceries']!));
      if (scene == 'make') _making(f, model);
      await s.tester.pumpWidget(
        s.app(
          SplitShell(
            model: model,
            home: const Material(child: _Grid()),
            detailFor: scene == 'make' ? _makeDetail(f) : _detail,
          ),
        ),
      );
      await s.settle(4);
      if (scene == 'make') {
        await _stream(s.tester, f);
        await s.settle(4);
      }
      await s.take('split-$scene');
      model.dispose();
      f.dispose();
    }, forms: const [wide]);
  }
}
