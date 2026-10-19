import 'package:clax_app/ui/mac/window_channel.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

final _mac = TargetPlatformVariant.only(TargetPlatform.macOS);

void main() {
  late List<MethodCall> calls;

  setUp(() {
    calls = [];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(MacWindow.channel, (call) async {
          calls.add(call);
          return null;
        });
  });

  tearDown(
    () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(MacWindow.channel, null),
  );

  /// [method] with [args] sent by the native side; answers its reply.
  Future<Object?> fromNative(
    WidgetTester tester,
    String method,
    Object? args,
  ) async {
    Object? reply;
    await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
      MacWindow.channel.name,
      MacWindow.channel.codec.encodeMethodCall(MethodCall(method, args)),
      (data) => reply = data == null
          ? null
          : MacWindow.channel.codec.decodeEnvelope(data),
    );
    return reply;
  }

  testWidgets('setTitle sends the title and the subtitle for the view', (
    tester,
  ) async {
    await MacWindow.setTitle('Groceries', subtitle: 'Version 4', view: 1);
    await MacWindow.setTitle('Clax', view: 1);
    expect(calls.map((c) => c.method), ['setTitle', 'setTitle']);
    expect(calls.map((c) => c.arguments), [
      {'view': 1, 'title': 'Groceries', 'subtitle': 'Version 4'},
      {'view': 1, 'title': 'Clax', 'subtitle': ''},
    ]);
  }, variant: _mac);

  testWidgets('setSidebarWidth sends the width in points for the view', (
    tester,
  ) async {
    await MacWindow.setSidebarWidth(220, view: 3);
    expect(calls.single.method, 'setSidebarWidth');
    expect(calls.single.arguments, {'view': 3, 'width': 220.0});
  }, variant: _mac);

  testWidgets('Escape is wanted while any owner in the view wants it, and '
      'each change is sent once', (tester) async {
    final a = Object(), b = Object();
    MacWindow.setEscapeWanted(a, true, view: 1);
    MacWindow.setEscapeWanted(a, true, view: 1);
    MacWindow.setEscapeWanted(b, true, view: 1);
    MacWindow.setEscapeWanted(a, false, view: 1);
    MacWindow.setEscapeWanted(b, false, view: 1);
    MacWindow.setEscapeWanted(b, false, view: 1);
    await tester.pump();
    expect(calls.map((c) => c.arguments), [
      {'view': 1, 'on': true},
      {'view': 1, 'on': false},
    ]);
  }, variant: _mac);

  testWidgets('an escape from the native side arrives with its view, and '
      'says whether that view wants it', (tester) async {
    final got = <int>[];
    final sub = MacWindow.escapes.listen(got.add);
    addTearDown(sub.cancel);
    final owner = Object();
    MacWindow.setEscapeWanted(owner, true, view: 1);
    addTearDown(() => MacWindow.setEscapeWanted(owner, false, view: 1));
    expect(await fromNative(tester, 'escape', {'view': 1}), isTrue);
    expect(await fromNative(tester, 'escape', {'view': 2}), isFalse);
    expect(await fromNative(tester, 'escape', 'nonsense'), isFalse);
    await tester.pump();
    expect(got, [1, 2]);
  }, variant: _mac);

  testWidgets('the title bar height arrives per view', (tester) async {
    final inset = MacWindow.titlebarHeight(5);
    expect(inset.value, 0);
    expect(await fromNative(tester, 'titlebar', {'view': 5, 'top': 28.0}),
        isTrue);
    expect(inset.value, 28);
    expect(MacWindow.titlebarHeight(6).value, 0);
  }, variant: _mac);

  testWidgets('the title bar height is asked for on first use', (
    tester,
  ) async {
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      MacWindow.channel,
      (call) async {
        calls.add(call);
        return call.method == 'titlebar' ? 52.0 : null;
      },
    );
    final inset = MacWindow.titlebarHeight(9);
    await tester.pump();
    expect(inset.value, 52);
    expect(MacWindow.titlebarHeight(9), same(inset));
    expect(calls.where((c) => c.method == 'titlebar').map((c) => c.arguments), [
      {'view': 9},
    ]);
  }, variant: _mac);

  testWidgets('nothing is sent off the Mac', (tester) async {
    await MacWindow.setTitle('Groceries', view: 1);
    await MacWindow.setSidebarWidth(220, view: 1);
    MacWindow.setEscapeWanted(Object(), true, view: 1);
    await tester.pump();
    expect(calls, isEmpty);
  }, variant: TargetPlatformVariant.only(TargetPlatform.android));

  testWidgets('a native side that does not answer is no error', (
    tester,
  ) async {
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      MacWindow.channel,
      (call) async => throw MissingPluginException(),
    );
    await MacWindow.setTitle('Groceries', view: 1);
    await MacWindow.setSidebarWidth(220, view: 1);
    expect(defaultTargetPlatform, TargetPlatform.macOS);
  }, variant: _mac);
}
