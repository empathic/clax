import 'package:clax_app/ui/kit/window_titlebar.dart';
import 'package:clax_app/ui/mac/window_channel.dart';
import 'package:clax_app/ui/mac/windows.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  late Map<int, String?> kinds;
  late List<int> asked;
  late List<Object?> opened;

  setUp(() {
    kinds = {};
    asked = [];
    opened = [];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(MacWindows.channel, (call) async {
          if (call.method == 'open') {
            opened.add(call.arguments);
            return true;
          }
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

  Future<void> pumpViews(WidgetTester tester, {WindowContents? further}) async {
    await tester.pumpWidget(
      ClaxViews(
        further: further,
        main: const Directionality(
          textDirection: TextDirection.ltr,
          child: _Counter(child: Text('main window')),
        ),
      ),
      wrapWithView: false,
    );
    await tester.pumpAndSettle();
  }

  testWidgets('a view whose window is Settings shows what the app gives '
      'that kind, from when it gives it', (tester) async {
    kinds[tester.view.viewId] = 'settings';
    final further = WindowContents();
    addTearDown(further.dispose);
    await pumpViews(tester, further: further);
    expect(find.text('main window'), findsNothing);
    further.set(
      'settings',
      (view) => Directionality(
        textDirection: TextDirection.ltr,
        child: Text('settings in $view'),
      ),
    );
    await tester.pump();
    expect(find.text('settings in ${tester.view.viewId}'), findsOneWidget);
    further.set('settings', null);
    await tester.pump();
    expect(find.textContaining('settings in'), findsNothing);
  });

  test('open asks the native side for the window of a kind, and answers '
      'whether one shows', () async {
    expect(await MacWindows.open('inbox'), isTrue);
    expect(opened, [
      {'kind': 'inbox'},
    ]);
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          MacWindows.channel,
          (call) async => throw MissingPluginException(),
        );
    expect(await MacWindows.open('settings'), isFalse);
  });

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
    final counter = tester.state(find.byType(_Counter));
    double top() =>
        MediaQuery.paddingOf(tester.element(find.text('main window'))).top;
    expect(top(), 0);
    await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
      MacWindow.channel.name,
      MacWindow.channel.codec.encodeMethodCall(
        MethodCall('titlebar', {
          'view': tester.view.viewId,
          'top': 52.0,
          'leading': 74.0,
        }),
      ),
      (_) {},
    );
    await tester.pump();
    expect(top(), 52);
    expect(
      MediaQuery.viewPaddingOf(tester.element(find.text('main window'))).top,
      52,
    );
    expect(
      WindowTitlebar.of(tester.element(find.text('main window'))),
      const WindowTitlebar(top: 52, leading: 74),
    );
    expect(
      tester.state(find.byType(_Counter)),
      same(counter),
      reason: 'the content keeps its state as the title bar arrives',
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

/// A widget with state, to see that it is kept.
class _Counter extends StatefulWidget {
  const _Counter({required this.child});

  final Widget child;

  @override
  State<_Counter> createState() => _CounterState();
}

class _CounterState extends State<_Counter> {
  @override
  Widget build(BuildContext context) => widget.child;
}
