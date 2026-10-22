import 'package:clax_app/ui/kit/family.dart';
import 'package:clax_app/ui/shortcuts.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

/// The command [shortcuts] maps [trigger] with exactly these modifiers to.
ClaxCommand? _commandOf(
  Map<ShortcutActivator, Intent> shortcuts,
  LogicalKeyboardKey trigger, {
  bool meta = false,
  bool control = false,
  bool shift = false,
  bool alt = false,
}) {
  for (final MapEntry(:key, :value) in shortcuts.entries) {
    if (key is SingleActivator &&
        key.trigger == trigger &&
        key.meta == meta &&
        key.control == control &&
        key.shift == shift &&
        key.alt == alt &&
        value is ClaxCommandIntent) {
      return value.command;
    }
  }
  return null;
}

void main() {
  group('claxShortcuts', () {
    test('the Apple family takes Command', () {
      final s = claxShortcuts(ClaxFamily.apple);
      ClaxCommand? cmd(
        LogicalKeyboardKey k, {
        bool shift = false,
        bool alt = false,
        bool control = false,
      }) => _commandOf(
        s,
        k,
        meta: true,
        shift: shift,
        alt: alt,
        control: control,
      );
      expect(cmd(LogicalKeyboardKey.keyK), ClaxCommand.toggleThreads);
      expect(cmd(LogicalKeyboardKey.keyK, shift: true), ClaxCommand.comment);
      expect(cmd(LogicalKeyboardKey.bracketLeft), ClaxCommand.back);
      expect(cmd(LogicalKeyboardKey.bracketRight), ClaxCommand.forward);
      expect(cmd(LogicalKeyboardKey.keyH, shift: true), ClaxCommand.home);
      expect(cmd(LogicalKeyboardKey.digit1), ClaxCommand.starred1);
      expect(cmd(LogicalKeyboardKey.digit9), ClaxCommand.starred9);
      expect(cmd(LogicalKeyboardKey.keyN), ClaxCommand.newApp);
      expect(cmd(LogicalKeyboardKey.keyO), ClaxCommand.importApp);
      expect(cmd(LogicalKeyboardKey.comma), ClaxCommand.settings);
      expect(cmd(LogicalKeyboardKey.keyI, shift: true), ClaxCommand.inbox);
      expect(cmd(LogicalKeyboardKey.keyI, alt: true), ClaxCommand.inspect);
      expect(
        cmd(LogicalKeyboardKey.keyS, control: true),
        ClaxCommand.toggleSidebar,
      );
      expect(cmd(LogicalKeyboardKey.backspace), ClaxCommand.deleteApp);
      // Nothing of Clax's takes Control alone on the Mac.
      expect(
        _commandOf(s, LogicalKeyboardKey.keyK, control: true),
        isNull,
      );
    });

    test('the Material family takes Control for each', () {
      final s = claxShortcuts(ClaxFamily.material);
      ClaxCommand? cmd(LogicalKeyboardKey k, {bool shift = false}) =>
          _commandOf(s, k, control: true, shift: shift);
      expect(cmd(LogicalKeyboardKey.keyK), ClaxCommand.toggleThreads);
      expect(cmd(LogicalKeyboardKey.keyK, shift: true), ClaxCommand.comment);
      expect(cmd(LogicalKeyboardKey.bracketLeft), ClaxCommand.back);
      expect(cmd(LogicalKeyboardKey.keyH, shift: true), ClaxCommand.home);
      expect(cmd(LogicalKeyboardKey.digit1), ClaxCommand.starred1);
      expect(_commandOf(s, LogicalKeyboardKey.keyK, meta: true), isNull);
    });

    test('Space opens the selected app in either family', () {
      for (final f in ClaxFamily.values) {
        expect(
          _commandOf(claxShortcuts(f), LogicalKeyboardKey.space),
          ClaxCommand.openSelected,
        );
      }
    });

    test('no two commands share a key', () {
      for (final f in ClaxFamily.values) {
        final keys = [
          for (final a in claxShortcuts(f).keys.cast<SingleActivator>())
            (a.trigger, a.meta, a.control, a.shift, a.alt),
        ];
        expect(keys.toSet(), hasLength(keys.length));
      }
    });

    testWidgets('a key press reaches the command through its intent', (
      tester,
    ) async {
      final ran = <ClaxCommand>[];
      await tester.pumpWidget(
        MaterialApp(
          home: Shortcuts(
            shortcuts: claxShortcuts(ClaxFamily.apple),
            child: Actions(
              actions: {
                ClaxCommandIntent: CallbackAction<ClaxCommandIntent>(
                  onInvoke: (i) => ran.add(i.command),
                ),
              },
              child: const Focus(autofocus: true, child: SizedBox()),
            ),
          ),
        ),
      );
      await tester.pump();
      await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyK);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyK);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
      expect(ran, [ClaxCommand.comment, ClaxCommand.toggleThreads]);
    });
  });

  test('each command keeps its menu tag (MainMenu.xib sends these)', () {
    expect({for (final c in ClaxCommand.values) c.name: c.tag}, {
      'newApp': 1,
      'importApp': 2,
      'shareApp': 3,
      'toggleSidebar': 4,
      'comment': 5,
      'toggleThreads': 6,
      'reload': 7,
      'actualSize': 8,
      'zoomIn': 9,
      'zoomOut': 10,
      'back': 11,
      'forward': 12,
      'home': 13,
      'inspect': 14,
      'versions': 15,
      'star': 16,
      'resumeBuild': 17,
      'inbox': 18,
      'settings': 19,
      'deleteApp': 20,
      'openSelected': 21,
      'starred1': 22,
      'starred2': 23,
      'starred3': 24,
      'starred4': 25,
      'starred5': 26,
      'starred6': 27,
      'starred7': 28,
      'starred8': 29,
      'starred9': 30,
    });
    expect(ClaxCommand.ofTag(19), ClaxCommand.settings);
    expect(ClaxCommand.ofTag(0), isNull);
    expect(ClaxCommand.ofTag(31), isNull);
  });

  group('MacMenu', () {
    late List<MethodCall> calls;
    setUp(() {
      calls = [];
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(MacMenu.channel, (call) async {
            calls.add(call);
            return null;
          });
    });
    tearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(MacMenu.channel, null),
    );

    testWidgets('sends enabled states and titles by tag on the Mac', (
      tester,
    ) async {
      MacMenu.setEnabled(ClaxCommand.settings, true);
      MacMenu.setTitle(ClaxCommand.star, 'Unstar');
      await tester.pump();
      expect(calls.map((c) => c.method), ['setEnabled', 'setTitle']);
      expect(calls.map((c) => c.arguments), [
        {'tag': 19, 'enabled': true},
        {'tag': 16, 'title': 'Unstar'},
      ]);
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets('an enabled state for one window names its view', (
      tester,
    ) async {
      MacMenu.setEnabled(ClaxCommand.settings, true, view: 2);
      await tester.pump();
      expect(calls.single.arguments, {'tag': 19, 'enabled': true, 'view': 2});
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets('a chosen item arrives with the view of the window it was '
        'chosen in', (tester) async {
      final got = <MenuChoice>[];
      final sub = MacMenu.choices.listen(got.add);
      addTearDown(sub.cancel);
      for (final args in <Object?>[
        {'tag': 13, 'view': 2},
        {'tag': 13, 'view': null},
        13,
        {'tag': 99, 'view': 2},
      ]) {
        await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
          MacMenu.channel.name,
          MacMenu.channel.codec.encodeMethodCall(MethodCall('command', args)),
          (_) {},
        );
      }
      await tester.pump();
      expect(got, [
        (command: ClaxCommand.home, view: 2),
        (command: ClaxCommand.home, view: null),
        (command: ClaxCommand.home, view: null),
      ]);
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets('sends nothing off the Mac', (tester) async {
      MacMenu.setEnabled(ClaxCommand.settings, true);
      await tester.pump();
      expect(calls, isEmpty);
    }, variant: TargetPlatformVariant.only(TargetPlatform.android));

    testWidgets('a chosen item arrives as its command', (tester) async {
      final got = <ClaxCommand>[];
      final sub = MacMenu.commands.listen(got.add);
      addTearDown(sub.cancel);
      Future<Object?> send(Object? tag) async {
        Object? reply;
        await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
          MacMenu.channel.name,
          MacMenu.channel.codec.encodeMethodCall(MethodCall('command', tag)),
          (data) => reply = data == null
              ? null
              : MacMenu.channel.codec.decodeEnvelope(data),
        );
        return reply;
      }

      expect(await send(13), isTrue);
      expect(await send(99), isFalse);
      await tester.pump();
      expect(got, [ClaxCommand.home]);
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
  });

}
