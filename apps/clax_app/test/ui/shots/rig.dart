// The render rig for the `*_shots_test.dart` files: each surface is rendered
// in three forms (the phone, Android wide and the Mac), light and dark, at
// the text scales a test asks for, with real fonts and shadows, into the
// directory CLAX_SHOTS_DIR names, as
// `<surface>-<form>-<theme>[-<scale×100>].png`. Without CLAX_SHOTS_DIR every
// render is skipped. Nothing compares pixels: the PNGs are for a person to
// look at.
//
//   SHOTS="$PWD/target/flutter-app/shots"; mkdir -p "$SHOTS"
//   bazelisk test //apps/clax_app:ui_shots_<surface>_shots_test \
//     --nocache_test_results --test_env=CLAX_SHOTS_DIR="$SHOTS" \
//     --sandbox_writable_path="$SHOTS"
import 'dart:io';

import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/tokens.g.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

/// A window a surface is rendered in: its size in logical pixels, its
/// device pixel ratio and platform, the system bars over its top and bottom
/// edges (Android), and the toolbar band over its top (the Mac).
class ShotForm {
  const ShotForm(
    this.name,
    this.size,
    this.dpr,
    this.platform, {
    this.top = 0,
    this.bottom = 0,
    this.toolbar = 0,
  });
  final String name;
  final Size size;
  final double dpr;
  final TargetPlatform platform;
  final double top, bottom, toolbar;
}

/// An Android phone: 412×915 dp at 2.625×, a 24 dp status bar and a 16 dp
/// gesture bar.
const phone = ShotForm(
  'phone',
  Size(412, 915),
  2.625,
  TargetPlatform.android,
  top: 24,
  bottom: 16,
);

/// An iPhone: 402×874 pt at 3×, a 62 pt status bar around the Dynamic
/// Island and a 34 pt home indicator.
const iphone = ShotForm(
  'iphone',
  Size(402, 874),
  3,
  TargetPlatform.iOS,
  top: 62,
  bottom: 34,
);

/// An Android tablet, foldable or Chromebook window: 1280×800 dp at 2×.
const wide = ShotForm(
  'wide',
  Size(1280, 800),
  2,
  TargetPlatform.android,
  top: 24,
  bottom: 16,
);

/// A Mac window: 1200×800 pt at 2×, under a 52 pt unified toolbar.
const mac = ShotForm(
  'mac',
  Size(1200, 800),
  2,
  TargetPlatform.macOS,
  toolbar: 52,
);

const allForms = [phone, wide, mac];

/// The directory renders are written to: CLAX_SHOTS_DIR, or null when it
/// is unset or empty.
String? get shotsDir => shotsDirFrom(Platform.environment);

/// [shotsDir] read from [env].
String? shotsDirFrom(Map<String, String> env) {
  final dir = env['CLAX_SHOTS_DIR'];
  return dir == null || dir.isEmpty ? null : dir;
}

/// The file name of [surface]'s render in [form], dark or light, at
/// [textScale]: the scale is named only when it is not 1.
String shotName(String surface, ShotForm form, bool dark, double textScale) {
  final scale = textScale == 1 ? '' : '-${(textScale * 100).round()}';
  return '$surface-${form.name}-${dark ? 'dark' : 'light'}$scale.png';
}

/// One render's test: the form, the theme and the text scale it draws in.
class Shot {
  Shot(this.tester, this.form, this.dark, this.textScale);
  final WidgetTester tester;
  final ShotForm form;
  final bool dark;
  final double textScale;

  /// [home] in the app's theme, under the form's chrome when [chrome]: the
  /// status and gesture bar stand-ins on Android, a toolbar band with the
  /// traffic lights and [title] on the Mac.
  Widget app(Widget home, {bool chrome = true, String title = 'Clax'}) =>
      MaterialApp(
        debugShowCheckedModeBanner: false,
        title: title,
        theme: claxTheme(Brightness.light, form.platform),
        darkTheme: claxTheme(Brightness.dark, form.platform),
        themeMode: dark ? ThemeMode.dark : ThemeMode.light,
        builder: !chrome
            ? null
            : form.toolbar > 0
            ? (context, child) => Column(
                children: [
                  MacToolbarBand(height: form.toolbar, title: title),
                  Expanded(
                    child: MediaQuery.removePadding(
                      context: context,
                      removeTop: true,
                      child: child!,
                    ),
                  ),
                ],
              )
            : (context, child) => Stack(
                children: [
                  child!,
                  SystemBars(top: form.top, bottom: form.bottom),
                ],
              ),
        home: home,
      );

  /// Pumps [frames] frames 100 ms apart, decodes every image on screen,
  /// and pumps once more.
  Future<void> settle([int frames = 8]) async {
    for (var i = 0; i < frames; i++) {
      await tester.pump(const Duration(milliseconds: 100));
    }
    await tester.runAsync(() async {
      for (final e in find.byType(Image).evaluate()) {
        try {
          await precacheImage((e.widget as Image).image, e);
        } catch (_) {
          // An image that does not decode is drawn as it would be.
        }
      }
    });
    await tester.pump(const Duration(milliseconds: 300));
  }

  /// Writes the render of [of] (the whole window by default) as
  /// [surface]'s PNG in [shotsDir].
  Future<void> take(String surface, {Finder? of}) async {
    await expectLater(
      of ?? find.byType(MaterialApp).first,
      matchesGoldenFile(shotName(surface, form, dark, textScale)),
    );
  }
}

/// Stand-ins for the status bar and the gesture bar (or home indicator)
/// over the insets.
class SystemBars extends StatelessWidget {
  const SystemBars({super.key, required this.top, required this.bottom});
  final double top;
  final double bottom;

  @override
  Widget build(BuildContext context) {
    final fg = Theme.of(context).brightness == Brightness.dark
        ? Colors.white
        : Colors.black;
    return IgnorePointer(
      child: Stack(
        children: [
          Positioned(
            left: 0,
            right: 0,
            top: 0,
            height: top,
            child: DefaultTextStyle(
              style: TextStyle(fontSize: 13, color: fg, fontFamily: 'Roboto'),
              child: Row(
                children: [
                  const SizedBox(width: 18),
                  const Text('9:41'),
                  const Spacer(),
                  Icon(Icons.signal_cellular_alt, size: 14, color: fg),
                  Icon(Icons.wifi, size: 14, color: fg),
                  Icon(Icons.battery_full, size: 14, color: fg),
                  const SizedBox(width: 14),
                ],
              ),
            ),
          ),
          Positioned(
            left: 0,
            right: 0,
            bottom: (bottom - 4) / 2,
            child: Center(
              child: Container(
                width: 108,
                height: 4,
                decoration: BoxDecoration(
                  color: fg.withValues(alpha: 0.75),
                  borderRadius: BorderRadius.circular(2),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// A stand-in for the Mac window's unified toolbar, which `flutter_test`
/// cannot draw: the traffic lights, the window's title, and a hairline
/// under them, in the brand's surface colours.
class MacToolbarBand extends StatelessWidget {
  const MacToolbarBand({super.key, required this.height, required this.title});
  final double height;
  final String title;

  @override
  Widget build(BuildContext context) {
    final dark = Theme.of(context).brightness == Brightness.dark;
    final p = dark ? ClaxTokens.dark : ClaxTokens.light;
    Widget light(Color c) => Container(
      width: 12,
      height: 12,
      margin: const EdgeInsets.only(right: 8),
      decoration: BoxDecoration(
        color: c,
        shape: BoxShape.circle,
        border: Border.all(color: Colors.black.withValues(alpha: 0.12)),
      ),
    );
    return Material(
      color: p.card,
      child: Container(
      height: height,
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: p.border)),
      ),
      padding: const EdgeInsets.only(left: 20),
      child: Row(
        children: [
          light(const Color(0xFFFF5F57)),
          light(const Color(0xFFFEBC2E)),
          light(const Color(0xFF28C840)),
          const SizedBox(width: 20),
          Text(
            title,
            style: ClaxType.of(
              ClaxRole.headline,
              TargetPlatform.macOS,
            ).copyWith(color: p.fg),
          ),
        ],
      ),
      ),
    );
  }
}

var _fontsLoaded = false;

/// Loads the fonts the forms draw in, once per test process: Roboto and
/// the Material icons from Flutter's material fonts; SF Pro as the Apple
/// system font, SF Mono and Menlo; New York; and stand-ins for Android's
/// `serif` (Georgia) and `monospace` (Andale Mono).
Future<void> loadFonts() async {
  if (_fontsLoaded) return;
  final material = _materialFonts();
  Future<ByteData> f(String path) async =>
      ByteData.sublistView(await File(path).readAsBytes());
  Future<void> load(String family, List<String> paths) {
    final loader = FontLoader(family);
    for (final p in paths) {
      loader.addFont(f(p));
    }
    return loader.load();
  }

  const sys = '/System/Library/Fonts';
  const sup = '$sys/Supplemental';
  await load('Roboto', [
    for (final w in ['Regular', 'Italic', 'Medium', 'Bold'])
      '$material/Roboto-$w.ttf',
  ]);
  await load('MaterialIcons', ['$material/MaterialIcons-Regular.otf']);
  for (final family in [
    '.AppleSystemUIFont',
    'CupertinoSystemText',
    'CupertinoSystemDisplay',
  ]) {
    await load(family, ['$sys/SFNS.ttf', '$sys/SFNSItalic.ttf']);
  }
  await load('.AppleSystemUIFontMonospaced', ['$sys/SFNSMono.ttf']);
  await load('Menlo', ['$sys/Menlo.ttc']);
  for (final family in ['.New York', 'New York']) {
    await load(family, ['$sys/NewYork.ttf', '$sys/NewYorkItalic.ttf']);
  }
  await load('serif', ['$sup/Georgia.ttf', '$sup/Georgia Italic.ttf']);
  await load('Georgia', ['$sup/Georgia.ttf', '$sup/Georgia Italic.ttf']);
  await load('monospace', ['$sup/Andale Mono.ttf']);
  _fontsLoaded = true;
}

/// Flutter's material fonts: under FLUTTER_ROOT when `flutter test` runs
/// the file, else beside the `flutter_tester` running it (Bazel's Flutter
/// repository keeps them in `material-fonts/`).
String _materialFonts() {
  final root = Platform.environment['FLUTTER_ROOT'];
  final tester = File(Platform.resolvedExecutable).resolveSymbolicLinksSync();
  final candidates = [
    if (root != null) '$root/bin/cache/artifacts/material_fonts',
    '${File(tester).parent.parent.path}/material-fonts',
  ];
  for (final dir in candidates) {
    if (File('$dir/Roboto-Regular.ttf').existsSync()) return dir;
  }
  throw StateError('no material fonts in any of $candidates');
}

/// Declares one test per form, theme and text scale, running [body] in
/// that window; each is skipped when [shotsDir] is null.
void shots(
  String surface,
  Future<void> Function(Shot s) body, {
  List<ShotForm> forms = allForms,
  List<double> textScales = const [1],
}) {
  final dir = shotsDir;
  for (final form in forms) {
    for (final dark in [false, true]) {
      for (final scale in textScales) {
        final name = shotName(surface, form, dark, scale);
        testWidgets(
          name.substring(0, name.length - '.png'.length),
          skip: dir == null,
          (tester) async {
            final s = Shot(tester, form, dark, scale);
            await tester.runAsync(loadFonts);
            final comparator = goldenFileComparator;
            final update = autoUpdateGoldenFiles;
            goldenFileComparator = LocalFileComparator(
              Uri.file('${dir!}/x.png'),
            );
            autoUpdateGoldenFiles = true;
            tester.view.physicalSize = form.size * form.dpr;
            tester.view.devicePixelRatio = form.dpr;
            final inset = FakeViewPadding(
              top: form.top * form.dpr,
              bottom: form.bottom * form.dpr,
            );
            tester.view.padding = inset;
            tester.view.viewPadding = inset;
            tester.platformDispatcher.platformBrightnessTestValue = dark
                ? Brightness.dark
                : Brightness.light;
            tester.platformDispatcher.textScaleFactorTestValue = scale;
            addTearDown(tester.view.reset);
            addTearDown(
              tester.platformDispatcher.clearPlatformBrightnessTestValue,
            );
            addTearDown(
              tester.platformDispatcher.clearTextScaleFactorTestValue,
            );
            debugDefaultTargetPlatformOverride = form.platform;
            debugDisableShadows = false;
            try {
              await body(s);
            } finally {
              // The binding checks these before the test's tear-downs run.
              goldenFileComparator = comparator;
              autoUpdateGoldenFiles = update;
              debugDisableShadows = true;
              debugDefaultTargetPlatformOverride = null;
            }
          },
        );
      }
    }
  }
}
