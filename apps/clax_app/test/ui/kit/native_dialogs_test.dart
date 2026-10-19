import 'dart:async';

import 'package:clax_app/ui/kit/button.dart';
import 'package:clax_app/ui/kit/dialog.dart';
import 'package:clax_app/ui/kit/menu.dart';
import 'package:clax_app/ui/kit/native_dialogs.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

/// The calls the fake `dev.empathic.clax/dialogs` handler saw, answered
/// by [answer].
class _FakeDialogs {
  _FakeDialogs(this.answer) {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(NativeDialogs.channel, (call) async {
          calls.add(call);
          return answer(call);
        });
    addTearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(NativeDialogs.channel, null),
    );
  }

  final FutureOr<Object?> Function(MethodCall call) answer;
  final calls = <MethodCall>[];
}

Widget _page(void Function(BuildContext context) onGo) => MaterialApp(
  theme: claxTheme(Brightness.light, defaultTargetPlatform),
  home: Scaffold(
    body: Builder(
      builder: (context) => Center(
        child: TextButton(onPressed: () => onGo(context), child: const Text('Go')),
      ),
    ),
  ),
);

final _mac = TargetPlatformVariant.only(TargetPlatform.macOS);

void main() {
  group('NativeDialogs.menu', () {
    test('sends each item and separator, and the point', () async {
      final fake = _FakeDialogs((_) => 'copy');
      final got = await NativeDialogs.menu(const [
        ClaxMenuItem('copy', 'Copy', key: '⌘C', symbol: 'doc.on.doc'),
        ClaxMenuItem.separator(),
        ClaxMenuItem('delete', 'Delete', destructive: true, enabled: false),
        ClaxMenuItem('pick', 'Opus', detail: 'claude-opus', checked: true),
      ], const Offset(12, 34.5));
      expect(got, 'copy');
      expect(fake.calls.single.method, 'menu');
      expect(fake.calls.single.arguments, {
        'items': [
          {
            'id': 'copy',
            'title': 'Copy',
            'key': '⌘C',
            'symbol': 'doc.on.doc',
            'detail': null,
            'checked': false,
            'destructive': false,
            'enabled': true,
          },
          {'separator': true},
          {
            'id': 'delete',
            'title': 'Delete',
            'key': null,
            'symbol': null,
            'detail': null,
            'checked': false,
            'destructive': true,
            'enabled': false,
          },
          {
            'id': 'pick',
            'title': 'Opus',
            'key': null,
            'symbol': null,
            'detail': 'claude-opus',
            'checked': true,
            'destructive': false,
            'enabled': true,
          },
        ],
        'at': {'x': 12.0, 'y': 34.5},
      });
    });

    test('answers null when nothing was chosen', () async {
      _FakeDialogs((_) => null);
      expect(
        await NativeDialogs.menu(const [ClaxMenuItem('a', 'A')], Offset.zero),
        isNull,
      );
    });
  });

  group('NativeDialogs.alert', () {
    test('sends every argument and decodes the answer', () async {
      final fake = _FakeDialogs((_) => {'index': 1, 'suppressed': true});
      final got = await NativeDialogs.alert(
        style: 'warning',
        title: 'Allow?',
        body: 'It wants the network.',
        buttons: const ['Don’t allow', 'Allow once'],
        suppressedButtons: const [null, 'Always allow'],
        destructive: const [],
        defaultIndex: 1,
        cancelIndex: 0,
        armMs: 500,
        suppression: 'Always for this app',
      );
      expect(got.index, 1);
      expect(got.suppressed, isTrue);
      final call = fake.calls.single;
      expect(call.method, 'alert');
      final args = call.arguments as Map;
      expect(args, containsPair('style', 'warning'));
      expect(args, containsPair('title', 'Allow?'));
      expect(args, containsPair('body', 'It wants the network.'));
      expect(args, containsPair('buttons', ['Don’t allow', 'Allow once']));
      expect(args, containsPair('suppressedButtons', [null, 'Always allow']));
      expect(args, containsPair('destructive', <int>[]));
      expect(args, containsPair('defaultIndex', 1));
      expect(args, containsPair('cancelIndex', 0));
      expect(args, containsPair('armMs', 500));
      expect(args, containsPair('suppression', 'Always for this app'));
      expect(args['id'], isA<int>());
    });

    test('a completed dismissed asks the Mac to take the alert down', () async {
      final answer = Completer<Object?>();
      final fake = _FakeDialogs(
        (call) => call.method == 'alert' ? answer.future : null,
      );
      final dismissed = Completer<void>();
      final got = NativeDialogs.alert(
        style: 'informational',
        title: 'Stop?',
        buttons: const ['Keep', 'Stop'],
        cancelIndex: 0,
        dismissed: dismissed.future,
      );
      await pumpEventQueue();
      dismissed.complete();
      await pumpEventQueue();
      expect(fake.calls.map((c) => c.method), ['alert', 'dismissAlert']);
      expect(fake.calls[1].arguments, {'id': (fake.calls[0].arguments as Map)['id']});
      answer.complete({'index': 0, 'suppressed': false});
      expect((await got).index, 0);
    });
  });

  group('the kit on macOS', () {
    testWidgets('ClaxDialog.ask is an NSAlert, buttons in reading order', (
      tester,
    ) async {
      final fake = _FakeDialogs((_) => {'index': 1, 'suppressed': true});
      ClaxDialogAnswer? got;
      await tester.pumpWidget(
        _page((context) async {
          got = await ClaxDialog.ask(
            context,
            title: 'Allow reading?',
            body: 'Notes wants to read Tasks.',
            suppression: 'Always for this app',
            actions: const [
              ClaxDialogAction('Don’t allow'),
              ClaxDialogAction(
                'Allow once',
                kind: ClaxButtonKind.primary,
                suppressedLabel: 'Always allow',
              ),
            ],
            defaultIndex: 1,
            cancelIndex: 0,
          );
        }),
      );
      await tester.tap(find.text('Go'));
      await tester.pumpAndSettle();
      expect(find.text('Allow reading?'), findsNothing);
      expect(got?.index, 1);
      expect(got?.suppressed, isTrue);
      final args = fake.calls.single.arguments as Map;
      expect(args['style'], 'informational');
      expect(args['buttons'], ['Don’t allow', 'Allow once']);
      expect(args['suppressedButtons'], [null, 'Always allow']);
      expect(args['suppression'], 'Always for this app');
      expect(args['defaultIndex'], 1);
      expect(args['cancelIndex'], 0);
      expect(args['armMs'], isNull);
    }, variant: _mac);

    testWidgets('an armed dialog sends armMs', (tester) async {
      final fake = _FakeDialogs((_) => {'index': 0, 'suppressed': false});
      await tester.pumpWidget(
        _page((context) {
          unawaited(
            ClaxDialog.ask(
              context,
              title: 'Allow?',
              actions: const [
                ClaxDialogAction('Don’t allow'),
                ClaxDialogAction('Allow', kind: ClaxButtonKind.primary),
              ],
              defaultIndex: 1,
              arming: const ClaxArming(
                delay: Duration(milliseconds: 500),
                waiting: 'Allow in a moment…',
              ),
            ),
          );
        }),
      );
      await tester.tap(find.text('Go'));
      await tester.pumpAndSettle();
      expect((fake.calls.single.arguments as Map)['armMs'], 500);
    }, variant: _mac);

    testWidgets('a destructive action is a warning and never the default', (
      tester,
    ) async {
      final fake = _FakeDialogs((_) => {'index': 0, 'suppressed': false});
      int? got = -1;
      await tester.pumpWidget(
        _page((context) async {
          got = await ClaxDialog.show(
            context,
            title: 'Delete this app?',
            actions: const [
              ClaxDialogAction('Cancel'),
              ClaxDialogAction('Delete', kind: ClaxButtonKind.destructive),
            ],
            defaultIndex: 1,
            cancelIndex: 0,
          );
        }),
      );
      await tester.tap(find.text('Go'));
      await tester.pumpAndSettle();
      expect(got, 0);
      final args = fake.calls.single.arguments as Map;
      expect(args['style'], 'warning');
      expect(args['destructive'], [1]);
      expect(args['defaultIndex'], isNull);
    }, variant: _mac);

    testWidgets('a dialog with content stays Flutter-drawn', (tester) async {
      final fake = _FakeDialogs((_) => {'index': 0, 'suppressed': false});
      await tester.pumpWidget(
        _page((context) {
          unawaited(
            ClaxDialog.ask(
              context,
              title: 'Pick one',
              content: const Text('radios'),
              actions: const [ClaxDialogAction('OK')],
            ),
          );
        }),
      );
      await tester.tap(find.text('Go'));
      await tester.pumpAndSettle();
      expect(find.text('Pick one'), findsOneWidget);
      expect(fake.calls, isEmpty);
    }, variant: _mac);

    testWidgets('with nothing on the channel the dialog is Flutter-drawn', (
      tester,
    ) async {
      _FakeDialogs((_) => throw MissingPluginException());
      await tester.pumpWidget(
        _page((context) {
          unawaited(
            ClaxDialog.show(
              context,
              title: 'Stop?',
              actions: const [ClaxDialogAction('OK')],
            ),
          );
        }),
      );
      await tester.tap(find.text('Go'));
      await tester.pumpAndSettle();
      expect(find.text('Stop?'), findsOneWidget);
    }, variant: _mac);

    testWidgets('ClaxMenu.show is an NSMenu below the anchor', (tester) async {
      final fake = _FakeDialogs((_) => 'b');
      String? got;
      await tester.pumpWidget(
        _page((context) async {
          got = await ClaxMenu.show(
            context,
            items: const [ClaxMenuItem('a', 'A'), ClaxMenuItem('b', 'B')],
            anchor: const Rect.fromLTWH(10, 20, 100, 40),
          );
        }),
      );
      await tester.tap(find.text('Go'));
      await tester.pumpAndSettle();
      expect(got, 'b');
      final args = fake.calls.single.arguments as Map;
      expect(args['at'], {'x': 10.0, 'y': 60.0});
      expect(find.byType(MenuItemButton), findsNothing);
    }, variant: _mac);
  });

  group('the kit on Android', () {
    testWidgets('never calls the channel', (tester) async {
      final fake = _FakeDialogs((_) => null);
      await tester.pumpWidget(
        _page((context) {
          unawaited(
            ClaxDialog.show(
              context,
              title: 'Stop?',
              actions: const [ClaxDialogAction('OK')],
            ),
          );
          unawaited(
            ClaxMenu.show(
              context,
              items: const [ClaxMenuItem('a', 'Menu A')],
              anchor: const Rect.fromLTWH(10, 20, 100, 40),
            ),
          );
        }),
      );
      await tester.tap(find.text('Go'));
      await tester.pumpAndSettle();
      expect(find.text('Stop?'), findsOneWidget);
      expect(fake.calls, isEmpty);
    });
  });
}
