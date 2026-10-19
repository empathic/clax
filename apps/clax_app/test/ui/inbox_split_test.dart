import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/ui/developer.dart';
import 'package:clax_app/ui/inbox/inbox_split.dart';
import 'package:clax_app/ui/inbox_screen.dart';
import 'package:clax_app/ui/mac/mac_toolbar.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/thread_detail.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_voice.dart';

const _page = {'index.html': '<!doctype html><title>x</title>'};

typedef _Opened = ({String aid, String? thread, int? version});

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  late FakeClaxApi api;
  late String aid;
  late String tid;
  late List<_Opened> opened;
  late List<String> questions;
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  late List<MethodCall> toolbar;

  setUp(() async {
    api = FakeClaxApi(clock: () => '2026-10-12T09:00:00Z');
    aid = api.seedArtifact('Run log', _page);
    await api.registerAppSession(installId: 'install', pid: 1);
    tid = api.addThread(aid, 'the weekly sum is off');
    api.pushInbox('reply', artifactId: aid, threadId: tid, body: 'Fixed it');
    api.pushInbox('version', artifactId: aid, versionN: 5, note: 'Weekly sum');
    opened = [];
    questions = [];
    toolbar = [];
    MacToolbar.reset();
    messenger.setMockMethodCallHandler(MacToolbar.channel, (call) async {
      toolbar.add(call);
      return null;
    });
  });

  tearDown(() => messenger.setMockMethodCallHandler(MacToolbar.channel, null));

  Future<void> pump(
    WidgetTester tester,
    double width, {
    bool windowToolbar = false,
  }) async {
    tester.view.physicalSize = Size(width, 700);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    final dev = ValueNotifier(false);
    addTearDown(dev.dispose);
    await tester.pumpWidget(
      DeveloperScope(
        notifier: dev,
        child: MaterialApp(
          theme: claxTheme(Brightness.light, defaultTargetPlatform),
          home: InboxSplit(
            api: api,
            stream: api.stream(),
            voice: FakeVoice(),
            now: () => DateTime.utc(2026, 10, 12, 9, 30),
            toolbar: windowToolbar,
            onOpenApp: (aid, {thread, version, comment}) =>
                opened.add((aid: aid, thread: thread, version: version)),
            onOpenQuestion: questions.add,
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
  }

  testWidgets('at 900 the list is 360 wide and a row shows its thread '
      'beside it', (tester) async {
    await pump(tester, 900);
    final list = find.byKey(const ValueKey('inbox-list-pane'));
    expect(tester.getSize(list).width, 360);
    expect(find.byType(ThreadDetail), findsNothing);
    expect(find.text('Nothing selected'), findsOneWidget);

    await tester.tap(find.text('Clax replied: Fixed it'));
    await tester.pumpAndSettle();
    final thread = find.byType(ThreadDetail);
    expect(thread, findsOneWidget);
    expect(tester.widget<ThreadDetail>(thread).tid, tid);
    expect(
      tester.getTopLeft(thread).dx,
      greaterThanOrEqualTo(360),
      reason: 'beside the list, not over it',
    );
    expect(list, findsOneWidget);

    await tester.tap(find.text('Open app'));
    await tester.pumpAndSettle();
    expect(opened, [(aid: aid, thread: tid, version: null)]);
  });

  testWidgets('at 900 a version shows what it is, and Open app opens the app '
      'at it', (tester) async {
    await pump(tester, 900);
    await tester.tap(find.text('Version 5 is ready: Weekly sum'));
    await tester.pumpAndSettle();
    expect(find.byType(ThreadDetail), findsNothing);
    await tester.tap(find.text('Open app'));
    await tester.pumpAndSettle();
    expect(opened, [(aid: aid, thread: null, version: 5)]);
  });

  testWidgets('at 412 a row pushes its thread', (tester) async {
    await pump(tester, 412);
    expect(find.byKey(const ValueKey('inbox-list-pane')), findsNothing);
    await tester.tap(find.text('Clax replied: Fixed it'));
    await tester.pumpAndSettle();
    expect(find.byType(ThreadDetail), findsOneWidget);
    expect(find.text('Clax replied: Fixed it'), findsNothing);
    await tester.pageBack();
    await tester.pumpAndSettle();
    expect(find.text('Clax replied: Fixed it'), findsOneWidget);
  });

  testWidgets('in the Mac Inbox window the toolbar holds the search and All, '
      'Unread and Questions, and a choice there filters the list', (
    tester,
  ) async {
    await pump(tester, 900, windowToolbar: true);
    final items =
        (toolbar.lastWhere((c) => c.method == 'setItems').arguments
                as Map)['items']
            as List;
    final ids = [for (final i in items) (i as Map)['id']];
    expect(ids, containsAllInOrder(['all', 'unread', 'questions']));
    expect(ids, contains('search'));
    expect(find.byType(SearchBar), findsNothing, reason: 'the toolbar has it');
    expect(find.byType(FilterChip), findsNothing);

    await messenger.handlePlatformMessage(
      MacToolbar.channel.name,
      MacToolbar.channel.codec.encodeMethodCall(
        MethodCall('clicked', {'view': tester.view.viewId, 'id': 'questions'}),
      ),
      (_) {},
    );
    await tester.pumpAndSettle();
    expect(
      api.calls.lastWhere((c) => c.name == 'inbox').args['kinds'],
      InboxFilter.questions.kinds,
    );
    final on =
        (toolbar.lastWhere((c) => c.method == 'setItems').arguments
                as Map)['items']
            as List;
    expect([
      for (final i in on)
        if ((i as Map)['on'] == true) i['id'],
    ], ['questions']);
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
}
