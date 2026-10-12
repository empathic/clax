import 'dart:async';

import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/clax/models.dart';
import 'package:clax_app/clax/stream.dart';
import 'package:clax_app/ui/inbox_screen.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

const _page = {'index.html': '<!doctype html><title>x</title>'};

/// Timers the screen starts, fired by hand.
class _Timers {
  final List<(Duration, void Function(), _FakeTimer)> started = [];

  Timer call(Duration d, void Function() f) {
    final t = _FakeTimer();
    started.add((d, f, t));
    return t;
  }

  /// Runs the newest timer that is still active.
  void fireLast() {
    final (_, f, t) = started.lastWhere((s) => s.$3.isActive);
    t.cancel();
    f();
  }
}

class _FakeTimer implements Timer {
  var _active = true;
  @override
  bool get isActive => _active;
  @override
  int get tick => 0;
  @override
  void cancel() => _active = false;
}

void main() {
  late FakeClaxApi api;
  late String aid;
  late List<InboxItem> opened;
  late _Timers timers;

  setUp(() {
    api = FakeClaxApi(clock: () => '2026-10-12T09:00:00Z');
    aid = api.seedArtifact('Run log', _page);
    opened = [];
    timers = _Timers();
  });

  Future<void> pumpInbox(WidgetTester tester, {Brightness? brightness}) async {
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(brightness ?? Brightness.light),
        home: InboxScreen(
          api: api,
          onOpen: opened.add,
          timer: timers.call,
          now: () => DateTime.utc(2026, 10, 12, 9, 30),
        ),
      ),
    );
    await tester.pumpAndSettle();
  }

  Map<String, Object?>? lastInboxArgs() =>
      api.calls.lastWhere((c) => c.name == 'inbox').args;

  testWidgets('subscribes to the inbox before it fetches it', (tester) async {
    await pumpInbox(tester);
    final names = api.callNames;
    expect(names, contains('streamSubscribe'));
    expect(names.indexOf('streamSubscribe'), lessThan(names.indexOf('inbox')));
    final topics =
        api.calls.firstWhere((c) => c.name == 'streamSubscribe').args['topics']
            as List;
    expect(topics, containsAll(['inbox', 'questions']));
  });

  testWidgets('shows one row per kind with its own line', (tester) async {
    api.pushInbox('reply', artifactId: aid, threadId: 't1', body: 'Added it');
    api.pushInbox('version', artifactId: aid, versionN: 5, note: 'Weekly sum');
    api.pushInbox('published', artifactId: aid);
    api.pushInbox('finished', artifactId: aid, message: 'All three fixed');
    await pumpInbox(tester);
    expect(find.text('Added it'), findsOneWidget);
    expect(find.text('v5: Weekly sum'), findsOneWidget);
    expect(find.text('New app'), findsOneWidget);
    expect(find.text('Finished: All three fixed'), findsOneWidget);
    expect(find.text('Run log'), findsNWidgets(4));
  });

  testWidgets("a question's row shows its header", (tester) async {
    final s = await api.registerAppSession(installId: 'i', pid: 1);
    await api.askQuestion(s.id, [
      {
        'question': 'Which unit should distances use?',
        'header': 'Units',
        'options': [
          {'label': 'km', 'description': ''},
          {'label': 'mi', 'description': ''},
        ],
      },
    ], artifactId: aid);
    await pumpInbox(tester);
    expect(find.text('Units'), findsOneWidget);
  });

  testWidgets('shows the newest items first', (tester) async {
    api.pushInbox('finished', artifactId: aid, message: 'older');
    api.pushInbox('finished', artifactId: aid, message: 'newer');
    await pumpInbox(tester);
    final older = tester.getTopLeft(find.text('Finished: older'));
    final newer = tester.getTopLeft(find.text('Finished: newer'));
    expect(newer.dy, lessThan(older.dy));
  });

  testWidgets('Unread and the kind chips change the query', (tester) async {
    await pumpInbox(tester);
    expect(lastInboxArgs()!['read'], 'all');
    await tester.tap(find.widgetWithText(FilterChip, 'Unread'));
    await tester.pumpAndSettle();
    expect(lastInboxArgs()!['read'], 'unread');
    await tester.tap(find.widgetWithText(FilterChip, 'Replies'));
    await tester.pumpAndSettle();
    expect(lastInboxArgs()!['kinds'], ['reply']);
    await tester.tap(find.widgetWithText(FilterChip, 'Versions'));
    await tester.pumpAndSettle();
    expect(lastInboxArgs()!['kinds'], ['reply', 'version']);
    await tester.tap(find.widgetWithText(FilterChip, 'Replies'));
    await tester.pumpAndSettle();
    expect(lastInboxArgs()!['kinds'], ['version']);
  });

  testWidgets('search waits for typing to pause', (tester) async {
    await pumpInbox(tester);
    final before = api.callNames.where((n) => n == 'inbox').length;
    await tester.enterText(find.byType(TextField), 'run');
    await tester.pump();
    await tester.enterText(find.byType(TextField), 'runs');
    await tester.pump();
    expect(
      api.callNames.where((n) => n == 'inbox').length,
      before,
      reason: 'no fetch until the timer fires',
    );
    timers.fireLast();
    await tester.pumpAndSettle();
    expect(api.callNames.where((n) => n == 'inbox').length, before + 1);
    expect(lastInboxArgs()!['q'], 'runs');
  });

  testWidgets('scrolling to the end fetches the next page', (tester) async {
    for (var i = 0; i < 60; i++) {
      api.pushInbox('finished', artifactId: aid, message: 'item $i');
    }
    await pumpInbox(tester);
    expect(lastInboxArgs()!['before'], isNull);
    await tester.scrollUntilVisible(
      find.text('Finished: item 0'),
      400,
      scrollable: find.byType(Scrollable).first,
    );
    await tester.pumpAndSettle();
    final paged = api.calls.where(
      (c) => c.name == 'inbox' && c.args['before'] != null,
    );
    expect(paged, hasLength(1));
    expect(paged.single.args['before'], '11');
    expect(find.text('Finished: item 0'), findsOneWidget);
  });

  testWidgets('a tap marks the item read and opens it', (tester) async {
    final item = api.pushInbox(
      'reply',
      artifactId: aid,
      threadId: 't1',
      body: 'Added it',
    );
    await pumpInbox(tester);
    expect(find.bySemanticsLabel(RegExp(r'^Unread\. ')), findsOneWidget);
    await tester.tap(find.text('Added it'));
    await tester.pumpAndSettle();
    expect(
      api.calls.where((c) => c.name == 'markInboxRead').single.args['itemId'],
      item.id,
    );
    expect(opened.single.id, item.id);
    expect(find.bySemanticsLabel(RegExp(r'^Unread\. ')), findsNothing);
  });

  testWidgets('a gone item is marked read but not opened', (tester) async {
    final item = api.pushInbox('finished', artifactId: aid, message: 'done');
    await pumpInbox(tester);
    api.emitStream(
      StreamEvent(
        topic: 'inbox',
        type: 'inbox_item',
        data: {'topic': 'inbox', 'unread': 1, 'item': _goneJson(item)},
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('No longer available'));
    await tester.pumpAndSettle();
    expect(api.callNames, contains('markInboxRead'));
    expect(opened, isEmpty);
  });

  testWidgets('Mark all read sends the newest seq shown and the filter', (
    tester,
  ) async {
    api.pushInbox('reply', artifactId: aid, body: 'a');
    final newest = api.pushInbox('reply', artifactId: aid, body: 'b');
    await pumpInbox(tester);
    await tester.tap(find.widgetWithText(FilterChip, 'Replies'));
    await tester.pumpAndSettle();
    await tester.tap(find.byTooltip('Mark all read'));
    await tester.pumpAndSettle();
    final call = api.calls.lastWhere((c) => c.name == 'markAllInboxRead');
    expect(call.args['upto'], newest.seq);
    expect(call.args['filter'], {
      'kind': ['reply'],
    });
    expect(find.bySemanticsLabel(RegExp(r'^Unread\. ')), findsNothing);
  });

  testWidgets('a new item arrives live at the top', (tester) async {
    api.pushInbox('finished', artifactId: aid, message: 'first');
    await pumpInbox(tester);
    api.pushInbox('reply', artifactId: aid, body: 'Live reply');
    await tester.pumpAndSettle();
    expect(find.text('Live reply'), findsOneWidget);
    final live = tester.getTopLeft(find.text('Live reply'));
    final first = tester.getTopLeft(find.text('Finished: first'));
    expect(live.dy, lessThan(first.dy));
  });

  testWidgets('an item event updates its row in place', (tester) async {
    final item = api.pushInbox('reply', artifactId: aid, body: 'Hello');
    await pumpInbox(tester);
    expect(find.bySemanticsLabel(RegExp(r'^Unread\. ')), findsOneWidget);
    await api.markInboxRead(item.id);
    await tester.pumpAndSettle();
    expect(find.text('Hello'), findsOneWidget);
    expect(find.bySemanticsLabel(RegExp(r'^Unread\. ')), findsNothing);
  });

  testWidgets('a live item outside the kind filter is not added', (
    tester,
  ) async {
    await pumpInbox(tester);
    await tester.tap(find.widgetWithText(FilterChip, 'Versions'));
    await tester.pumpAndSettle();
    api.pushInbox('reply', artifactId: aid, body: 'Not a version');
    await tester.pumpAndSettle();
    expect(find.text('Not a version'), findsNothing);
  });

  testWidgets('resync and inbox_read refetch', (tester) async {
    await pumpInbox(tester);
    final before = api.callNames.where((n) => n == 'inbox').length;
    api.emitStream(
      const StreamEvent(
        topic: 'inbox',
        type: 'resync',
        data: {'topic': 'inbox', 'reason': 'behind'},
      ),
    );
    await tester.pumpAndSettle();
    expect(api.callNames.where((n) => n == 'inbox').length, before + 1);
    api.emitStream(
      const StreamEvent(
        topic: 'inbox',
        type: 'inbox_read',
        data: {'topic': 'inbox', 'ids': null, 'read': true, 'unread': 0},
      ),
    );
    await tester.pumpAndSettle();
    expect(api.callNames.where((n) => n == 'inbox').length, before + 2);
  });

  testWidgets('pull to refresh fetches again', (tester) async {
    api.pushInbox('reply', artifactId: aid, body: 'Hello');
    await pumpInbox(tester);
    final before = api.callNames.where((n) => n == 'inbox').length;
    await tester.fling(find.text('Hello'), const Offset(0, 400), 1000);
    await tester.pumpAndSettle();
    expect(api.callNames.where((n) => n == 'inbox').length, before + 1);
  });

  testWidgets('an empty inbox says so', (tester) async {
    await pumpInbox(tester);
    expect(find.text('Nothing here yet'), findsOneWidget);
    await tester.tap(find.widgetWithText(FilterChip, 'Unread'));
    await tester.pumpAndSettle();
    expect(find.text("You're all caught up"), findsOneWidget);
  });

  testWidgets('a failed load offers Try again', (tester) async {
    api.failNext('inbox', StateError('down'));
    await pumpInbox(tester);
    expect(find.text("Couldn't load the inbox"), findsOneWidget);
    api.pushInbox('reply', artifactId: aid, body: 'Back');
    await tester.tap(find.text('Try again'));
    await tester.pumpAndSettle();
    expect(find.text('Back'), findsOneWidget);
  });

  testWidgets('the open inbox is an owner surface until it closes', (
    tester,
  ) async {
    final s = await api.registerAppSession(installId: 'i', pid: 1);
    Future<bool> surfaceOpen() async => (await api.askQuestion(s.id, [
      {'question': 'Which?', 'header': 'Pick'},
    ])).surfaceOpen;
    await pumpInbox(tester);
    expect(await surfaceOpen(), isTrue);
    await tester.pumpWidget(const SizedBox());
    await tester.pumpAndSettle();
    expect(await surfaceOpen(), isFalse);
  });

  test('times read as a phone inbox shows them', () {
    final now = DateTime.utc(2026, 10, 12, 9, 30);
    expect(inboxTime('2026-10-12T09:29:40Z', now), 'now');
    expect(inboxTime('2026-10-12T09:25:00Z', now), '5m');
    expect(inboxTime('2026-10-12T06:30:00Z', now), '3h');
    expect(inboxTime('2026-10-09T06:30:00Z', now), 'Fri');
    expect(inboxTime('2026-09-03T06:30:00Z', now), 'Sep 3');
    expect(inboxTime('2025-09-03T06:30:00Z', now), 'Sep 3, 2025');
    expect(inboxTime('garbage', now), '');
  });
}

Map<String, Object?> _goneJson(InboxItem item) => {
  'id': item.id,
  'seq': item.seq,
  'kind': item.kind,
  'read': false,
  'created_at': item.createdAt,
  'url': item.url,
  'gone': true,
  'artifact': null,
  'agent': null,
};
