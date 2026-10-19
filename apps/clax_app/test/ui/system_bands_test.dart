import 'package:clax_app/ui/system_bands.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/tokens.g.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  /// A phone of 360 x 780 with a 24 px status bar and a 16 px gesture bar.
  void phone(WidgetTester tester) {
    tester.view.devicePixelRatio = 3;
    tester.view.physicalSize = const Size(360, 780) * 3;
    tester.view.padding = const FakeViewPadding(top: 72, bottom: 48);
    addTearDown(tester.view.reset);
  }

  Future<void> pump(
    WidgetTester tester, {
    Color? top,
    Color? bottom,
    Brightness brightness = Brightness.light,
  }) => tester.pumpWidget(
    MaterialApp(
      theme: claxTheme(brightness, TargetPlatform.android),
      home: SystemBands(
        top: top,
        bottom: bottom,
        child: const SizedBox.expand(key: ValueKey('page')),
      ),
    ),
  );

  /// The style the system bars take from the frame just drawn.
  SystemUiOverlayStyle? latest() => SystemChrome.latestStyle;

  testWidgets('the page has the screen\'s whole height', (tester) async {
    phone(tester);
    await pump(tester, top: const Color(0xFF102030), bottom: Colors.white);
    expect(tester.getRect(find.byKey(const ValueKey('page'))), Offset.zero & const Size(360, 780));
  });

  testWidgets('the bands are painted in the page\'s colours over its insets', (
    tester,
  ) async {
    phone(tester);
    await pump(tester, top: const Color(0xFF102030), bottom: Colors.white);
    final top = tester.getRect(find.byKey(const ValueKey('system-band-top')));
    final bottom = tester.getRect(
      find.byKey(const ValueKey('system-band-bottom')),
    );
    expect(top, const Rect.fromLTWH(0, 0, 360, 24));
    expect(bottom, const Rect.fromLTWH(0, 764, 360, 16));
    Color colorOf(String key) =>
        tester.widget<ColoredBox>(find.byKey(ValueKey(key))).color;
    expect(colorOf('system-band-top'), const Color(0xFF102030));
    expect(colorOf('system-band-bottom'), Colors.white);
  });

  testWidgets('a dark top gives light status icons, a white bottom dark '
      'navigation icons', (tester) async {
    phone(tester);
    await pump(tester, top: const Color(0xFF102030), bottom: Colors.white);
    await tester.pump();
    expect(latest()!.statusBarIconBrightness, Brightness.light);
    expect(latest()!.systemNavigationBarIconBrightness, Brightness.dark);
  });

  testWidgets('a white top gives dark status icons', (tester) async {
    phone(tester);
    await pump(tester, top: Colors.white, bottom: const Color(0xFF102030));
    await tester.pump();
    expect(latest()!.statusBarIconBrightness, Brightness.dark);
    expect(latest()!.systemNavigationBarIconBrightness, Brightness.light);
  });

  testWidgets('before the page says, both bands are the palette\'s bg', (
    tester,
  ) async {
    phone(tester);
    await pump(tester, brightness: Brightness.dark);
    Color colorOf(String key) =>
        tester.widget<ColoredBox>(find.byKey(ValueKey(key))).color;
    expect(colorOf('system-band-top'), ClaxTokens.dark.bg);
    expect(colorOf('system-band-bottom'), ClaxTokens.dark.bg);
  });
}
