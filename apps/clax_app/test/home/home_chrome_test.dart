import 'package:clax_app/home/home_chrome.dart';
import 'package:clax_app/home/home_host.dart';
import 'package:clax_app/ui/kit/verbs.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  Widget app(TargetPlatform platform, List<HomeChrome> told) =>
      ClaxPointerScope(
        child: MaterialApp(
          theme: ThemeData(platform: platform),
          builder: (context, child) =>
              HomeChromeReport(onChrome: told.add, child: child!),
          home: const SizedBox.expand(),
        ),
      );

  testWidgets('a phone by touch, then wide with a mouse', (tester) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(400, 800);
    addTearDown(tester.view.reset);
    final told = <HomeChrome>[];
    await tester.pumpWidget(app(TargetPlatform.android, told));
    await tester.pump();
    expect(told, [
      (platform: 'android', form: 'phone', toolbar: false, pointer: 'touch'),
    ]);

    tester.view.physicalSize = const Size(900, 800);
    await tester.pump();
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: const Offset(10, 10));
    await mouse.moveTo(const Offset(20, 20));
    await tester.pump();
    await tester.pump();
    expect(told.last, (
      platform: 'android',
      form: 'wide',
      toolbar: false,
      pointer: 'mouse',
    ));
    final n = told.length;
    await mouse.moveTo(const Offset(30, 30));
    await tester.pump();
    await tester.pump();
    expect(told, hasLength(n), reason: 'an unchanged chrome is told once');
    await mouse.removePointer();
  });

  testWidgets('the Mac says mouse before any pointer', (tester) async {
    final told = <HomeChrome>[];
    await tester.pumpWidget(app(TargetPlatform.macOS, told));
    await tester.pump();
    expect(told.single.platform, 'macos');
    expect(told.single.pointer, 'mouse');
  });
}
