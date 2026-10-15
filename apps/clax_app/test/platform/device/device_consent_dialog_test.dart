// The device consent dialog's answers. Its screenshots, at phone width,
// light and dark, are off by default; run with
//   CLAX_L1_SHOTS=1 flutter test --update-goldens test/platform/device/device_consent_dialog_test.dart
// and look at target/flutter-app/l1/*.png.
import 'dart:io';

import 'package:clax_app/platform/device/device_consent_dialog.dart';
import 'package:clax_app/platform/device/device_host.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

final _shots = Platform.environment['CLAX_L1_SHOTS'] == '1';
const _out = '../../../../../target/flutter-app/l1';

Future<void> _fonts() async {
  final root = Platform.environment['FLUTTER_ROOT']!;
  final dir = '$root/bin/cache/artifacts/material_fonts';
  Future<ByteData> f(String n) async =>
      ByteData.sublistView(await File('$dir/$n').readAsBytes());
  await (FontLoader('Roboto')
        ..addFont(f('Roboto-Regular.ttf'))
        ..addFont(f('Roboto-Medium.ttf'))
        ..addFont(f('Roboto-Bold.ttf')))
      .load();
  await (FontLoader(
    'MaterialIcons',
  )..addFont(f('MaterialIcons-Regular.otf'))).load();
}

/// Shows the consent for [cap] over a page; the answer lands in the list.
Future<List<DeviceAnswer>> _show(
  WidgetTester tester,
  String cap, {
  bool dark = false,
}) async {
  final nav = GlobalKey<NavigatorState>();
  final answers = <DeviceAnswer>[];
  await tester.pumpWidget(
    MaterialApp(
      navigatorKey: nav,
      debugShowCheckedModeBanner: false,
      theme: claxTheme(dark ? Brightness.dark : Brightness.light),
      home: const Scaffold(body: Center(child: Text('Plant care'))),
    ),
  );
  final consent = DialogDeviceConsent(
    navigator: () => nav.currentState,
    title: (_) async => 'Plant care',
  );
  consent.ask('abcdefghjkmn', cap).then(answers.add);
  for (var i = 0; i < 6; i++) {
    await tester.pump(const Duration(milliseconds: 100));
  }
  return answers;
}

void main() {
  testWidgets('location offers Precise, Approximate and Don\'t allow, '
      'each saying how exactly', (tester) async {
    for (final (key, want) in const [
      ('consent-precise', DeviceAnswer.allow),
      ('consent-approximate', DeviceAnswer.approximate),
      ('consent-deny', DeviceAnswer.deny),
    ]) {
      final answers = await _show(tester, 'location');
      expect(
        find.text('Let “Plant care” see where your device is?'),
        findsOneWidget,
      );
      expect(
        find.text('Your exact location, within a few metres'),
        findsOneWidget,
      );
      expect(find.text('Only your area, within about 2 km'), findsOneWidget);
      await tester.tap(find.byKey(ValueKey(key)));
      await tester.pumpAndSettle();
      expect(answers, [want]);
    }
  });

  testWidgets('another capability offers Allow and Don\'t allow; '
      'dismissing is no answer', (tester) async {
    var answers = await _show(tester, 'camera');
    expect(find.byKey(const ValueKey('consent-approximate')), findsNothing);
    await tester.tap(find.text('Allow'));
    await tester.pumpAndSettle();
    expect(answers, [DeviceAnswer.allow]);
    answers = await _show(tester, 'camera');
    await tester.tapAt(const Offset(5, 5));
    await tester.pumpAndSettle();
    expect(answers, [DeviceAnswer.dismissed]);
  });

  for (final dark in [false, true]) {
    final scheme = dark ? 'dark' : 'light';
    testWidgets('location consent, phone, $scheme', skip: !_shots, (
      tester,
    ) async {
      await tester.runAsync(_fonts);
      tester.view.physicalSize = const Size(400, 860);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      await _show(tester, 'location', dark: dark);
      await expectLater(
        find.byType(MaterialApp),
        matchesGoldenFile('$_out/location-consent-phone-$scheme.png'),
      );
    });
  }
}
