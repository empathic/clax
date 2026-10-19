import 'package:clax_app/ui/mac/window_channel.dart';
import 'package:clax_app/ui/mac/windows.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  late Map<int, String?> kinds;
  late List<int> asked;

  setUp(() {
    kinds = {};
    asked = [];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(MacWindows.channel, (call) async {
          if (call.method != 'kind') return null;
          final view = call.arguments as int;
          asked.add(view);
          return kinds[view];
        });
  });

  tearDown(
    () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(MacWindows.channel, null),
  );

  Future<void> pumpViews(WidgetTester tester) async {
    await tester.pumpWidget(
      const ClaxViews(
        main: Directionality(
          textDirection: TextDirection.ltr,
          child: Text('main window'),
        ),
      ),
      wrapWithView: false,
    );
    await tester.pumpAndSettle();
  }

  testWidgets('the main window shows its content in the view the native '
      'side names as main', (tester) async {
    kinds[tester.view.viewId] = 'main';
    await pumpViews(tester);
    expect(asked, [tester.view.viewId]);
    expect(find.text('main window'), findsOneWidget);
  });

  testWidgets('a view no window shows (the engine\'s implicit view under '
      'multi-view) is left empty', (tester) async {
    await pumpViews(tester);
    expect(asked, [tester.view.viewId]);
    expect(find.text('main window'), findsNothing);
  });

  testWidgets('without the native side the implicit view is the main '
      'window', (tester) async {
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      MacWindows.channel,
      (call) async => throw MissingPluginException(),
    );
    await pumpViews(tester);
    expect(find.text('main window'), findsOneWidget);
  });

  testWidgets('the content is padded below the title bar the native side '
      'reports', (tester) async {
    kinds[tester.view.viewId] = 'main';
    await pumpViews(tester);
    double top() =>
        MediaQuery.paddingOf(tester.element(find.text('main window'))).top;
    expect(top(), 0);
    await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
      MacWindow.channel.name,
      MacWindow.channel.codec.encodeMethodCall(
        MethodCall('titlebar', {'view': tester.view.viewId, 'top': 28.0}),
      ),
      (_) {},
    );
    await tester.pump();
    expect(top(), 28);
    expect(
      MediaQuery.viewPaddingOf(tester.element(find.text('main window'))).top,
      28,
    );
  });

  testWidgets('a window that becomes key moves the focus into its view', (
    tester,
  ) async {
    kinds[tester.view.viewId] = 'main';
    MacWindows.install();
    await pumpViews(tester);
    Object? reply = 'unanswered';
    await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
      MacWindows.channel.name,
      MacWindows.channel.codec.encodeMethodCall(
        MethodCall('focused', {'view': tester.view.viewId}),
      ),
      (data) => reply = data == null
          ? null
          : MacWindows.channel.codec.decodeEnvelope(data),
    );
    expect(reply, isTrue);
  });
}
