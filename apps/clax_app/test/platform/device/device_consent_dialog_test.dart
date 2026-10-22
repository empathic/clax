// The device consent dialog's answers. Its renders are in
// test/ui/shots/sheets_shots_test.dart.
import 'package:clax_app/platform/device/device_consent_dialog.dart';
import 'package:clax_app/platform/device/device_host.dart';
import 'package:clax_app/ui/kit/kit.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

/// Shows the consent for [cap] over a page; the answer lands in the list.
Future<List<DeviceAnswer>> _show(
  WidgetTester tester,
  String cap,
) async {
  final nav = GlobalKey<NavigatorState>();
  final answers = <DeviceAnswer>[];
  await tester.pumpWidget(
    MaterialApp(
      key: UniqueKey(),
      navigatorKey: nav,
      debugShowCheckedModeBanner: false,
      theme: claxTheme(Brightness.light, defaultTargetPlatform),
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
  testWidgets('location asks Precise or Approximate as radios in the body, '
      'with Don’t allow and Allow', (tester) async {
    for (final (choice, want) in const [
      (null, DeviceAnswer.allow),
      ('consent-approximate', DeviceAnswer.approximate),
      ('consent-precise', DeviceAnswer.allow),
    ]) {
      final answers = await _show(tester, 'location');
      expect(
        find.text('Let “Plant care” see where your device is?'),
        findsOneWidget,
      );
      expect(find.byType(ClaxRadio<bool>), findsNWidgets(2));
      expect(
        find.text('Your exact location, within a few metres'),
        findsOneWidget,
      );
      expect(find.text('Only your area, within about 2 km'), findsOneWidget);
      expect(find.byType(ClaxButton), findsNWidgets(2));
      expect(find.text('Don’t allow'), findsOneWidget);
      if (choice != null) {
        await tester.tap(find.byKey(ValueKey(choice)));
        await tester.pump();
      }
      await tester.tap(find.text('Allow'));
      await tester.pumpAndSettle();
      expect(answers, [want]);
    }
    final answers = await _show(tester, 'location');
    await tester.tap(find.text('Don’t allow'));
    await tester.pumpAndSettle();
    expect(answers, [DeviceAnswer.deny]);
  });

  testWidgets('another capability offers Allow and Don’t allow; '
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
}
