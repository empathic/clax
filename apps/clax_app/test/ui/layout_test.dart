import 'package:clax_app/ui/kit/family.dart';
import 'package:clax_app/ui/layout.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('window size classes break at 600, 840 and 1200', () {
    expect(ClaxWidth.fromWidth(599), ClaxWidth.compact);
    expect(ClaxWidth.fromWidth(600), ClaxWidth.medium);
    expect(ClaxWidth.fromWidth(839), ClaxWidth.medium);
    expect(ClaxWidth.fromWidth(840), ClaxWidth.expanded);
    expect(ClaxWidth.fromWidth(1199), ClaxWidth.expanded);
    expect(ClaxWidth.fromWidth(1200), ClaxWidth.large);
  });

  test('size classes compare in order', () {
    expect(ClaxWidth.expanded >= ClaxWidth.medium, isTrue);
    expect(ClaxWidth.medium >= ClaxWidth.medium, isTrue);
    expect(ClaxWidth.compact >= ClaxWidth.medium, isFalse);
    expect(ClaxWidth.compact < ClaxWidth.medium, isTrue);
    expect(ClaxWidth.large < ClaxWidth.large, isFalse);
  });

  test('Apple is macOS and iOS, Material the rest', () {
    expect(ClaxFamily.forPlatform(TargetPlatform.android), ClaxFamily.material);
    expect(ClaxFamily.forPlatform(TargetPlatform.macOS), ClaxFamily.apple);
    expect(ClaxFamily.forPlatform(TargetPlatform.iOS), ClaxFamily.apple);
    expect(ClaxFamily.forPlatform(TargetPlatform.linux), ClaxFamily.material);
  });

  Future<(ClaxWidth, double, ClaxFamily)> read(
    WidgetTester tester,
    Size size,
    TargetPlatform platform,
  ) async {
    tester.view.physicalSize = size;
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    late (ClaxWidth, double, ClaxFamily) got;
    await tester.pumpWidget(
      MaterialApp(
        theme: ThemeData(platform: platform),
        themeAnimationDuration: Duration.zero,
        home: Builder(
          builder: (context) {
            got = (
              ClaxWidth.of(context),
              ClaxBreakpoints.gutter(context),
              ClaxFamily.of(context),
            );
            return const SizedBox();
          },
        ),
      ),
    );
    return got;
  }

  testWidgets('the size class and gutter follow the window, not the platform', (
    tester,
  ) async {
    expect(
      await read(tester, const Size(412, 915), TargetPlatform.android),
      (ClaxWidth.compact, 16.0, ClaxFamily.material),
    );
    expect(
      await read(tester, const Size(1280, 800), TargetPlatform.android),
      (ClaxWidth.large, 24.0, ClaxFamily.material),
    );
    expect(
      await read(tester, const Size(500, 800), TargetPlatform.macOS),
      (ClaxWidth.compact, 16.0, ClaxFamily.apple),
    );
    expect(
      await read(tester, const Size(1024, 768), TargetPlatform.iOS),
      (ClaxWidth.expanded, 24.0, ClaxFamily.apple),
    );
  });
}
