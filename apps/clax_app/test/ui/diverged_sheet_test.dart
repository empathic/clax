import 'package:clax_app/remote/merge.dart';
import 'package:clax_app/ui/bundle_sheets.dart';
import 'package:clax_app/ui/remote/diverged_sheet.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

/// Shows [sheet] from a button and answers what it popped.
Future<List<Object?>> _show(WidgetTester tester, Widget sheet) async {
  tester.view.physicalSize = const Size(400, 800);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  final answers = <Object?>[];
  await tester.pumpWidget(
    MaterialApp(
      key: UniqueKey(),
      home: Builder(
        builder: (context) => Scaffold(
          body: Center(
            child: TextButton(
              onPressed: () async =>
                  answers.add(await showBundleSheet<Object?>(context, sheet)),
              child: const Text('show'),
            ),
          ),
        ),
      ),
    ),
  );
  await tester.tap(find.text('show'));
  await tester.pumpAndSettle();
  expect(tester.takeException(), isNull);
  return answers;
}

Future<void> _tap(WidgetTester tester, String text) async {
  await tester.ensureVisible(find.text(text));
  await tester.pumpAndSettle();
  await tester.tap(find.text(text));
  await tester.pumpAndSettle();
}

void main() {
  const sheet = DivergedSheet(
    title: 'Plant log',
    host: 'clax.dev',
    who: '@sam',
    headVersion: 8,
    baseVersion: 7,
    canCombine: true,
  );

  testWidgets('says who moved the copy on and offers the three choices', (
    tester,
  ) async {
    var answers = await _show(tester, sheet);
    expect(find.text('clax.dev moved on'), findsOneWidget);
    expect(
      find.text('@sam published v8 after this device’s v7.'),
      findsOneWidget,
    );
    expect(
      find.text('Whichever you choose, this device’s versions stay in Versions.'),
      findsOneWidget,
    );
    await _tap(tester, 'Ask Claude to combine them');
    expect(answers, [DivergeChoice.combine]);

    answers = await _show(tester, sheet);
    await _tap(tester, 'Take the public v8 (drop mine)');
    expect(answers, [DivergeChoice.takeTheirs]);

    answers = await _show(tester, sheet);
    await _tap(tester, 'Publish mine as a new app');
    expect(answers, [DivergeChoice.newApp]);

    answers = await _show(tester, sheet);
    await _tap(tester, 'Not now');
    expect(answers, [null]);
  });

  testWidgets('names the head’s note when it has one', (tester) async {
    await _show(
      tester,
      const DivergedSheet(
        title: 'Plant log',
        host: 'clax.dev',
        who: '@sam',
        headVersion: 8,
        baseVersion: 7,
        note: 'Bigger buttons',
      ),
    );
    expect(
      find.text('@sam published v8 (“Bigger buttons”) after this device’s v7.'),
      findsOneWidget,
    );
  });

  testWidgets('without Claude there is no combine', (tester) async {
    await _show(
      tester,
      const DivergedSheet(
        title: 'Plant log',
        host: 'clax.dev',
        who: '@sam',
        headVersion: 8,
        baseVersion: 7,
      ),
    );
    expect(find.text('Ask Claude to combine them'), findsNothing);
    expect(find.text('Take the public v8 (drop mine)'), findsOneWidget);
    expect(find.text('Publish mine as a new app'), findsOneWidget);
  });

  testWidgets('a web-made version for an author: Update or Not now', (
    tester,
  ) async {
    final answers = await _show(
      tester,
      const WebVersionSheet(
        title: 'Plant log',
        host: 'clax.dev',
        who: '@sam',
        version: 8,
      ),
    );
    expect(find.text('Update Plant log to v8?'), findsOneWidget);
    expect(find.text('v8 by @sam, made on clax.dev'), findsOneWidget);
    await _tap(tester, 'Update');
    expect(answers, [true]);
  });
}
