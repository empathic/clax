import 'dart:async';

import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/ui/app_icon.dart';
import 'package:clax_app/ui/haiku.dart';
import 'package:clax_app/ui/inbox_screen.dart';
import 'package:clax_app/ui/kit/kit.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/foundation.dart';
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
  late List<Duration> delays;

  setUp(() {
    api = FakeClaxApi(clock: () => '2026-10-12T09:00:00Z');
    aid = api.seedArtifact('Run log', _page);
    opened = [];
    timers = _Timers();
    delays = [];
  });

  Future<void> pumpInbox(
    WidgetTester tester, {
    Brightness? brightness,
    List<String>? haikus,
  }) async {
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(brightness ?? Brightness.light, defaultTargetPlatform),
        home: InboxScreen(
          api: api,
          onOpen: opened.add,
          timer: timers.call,
          now: () => DateTime.utc(2026, 10, 12, 9, 30),
          retryDelay: (d) async => delays.add(d),
          haikus: haikus == null ? null : () async => haikus,
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
    expect(find.text('Clax replied: Added it'), findsOneWidget);
    expect(find.text('Version 5 is ready: Weekly sum'), findsOneWidget);
    expect(find.text('New app'), findsOneWidget);
    expect(find.text('Clax finished: All three fixed'), findsOneWidget);
    expect(find.text('Run log'), findsNWidgets(4));
  });

  testWidgets("a question's row says Clax asked it", (tester) async {
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
    expect(
      find.text('Clax asked: Which unit should distances use?'),
      findsOneWidget,
    );
  });

  testWidgets('shows the newest items first', (tester) async {
    api.pushInbox('finished', artifactId: aid, message: 'older');
    api.pushInbox('finished', artifactId: aid, message: 'newer');
    await pumpInbox(tester);
    final older = tester.getTopLeft(find.text('Clax finished: older'));
    final newer = tester.getTopLeft(find.text('Clax finished: newer'));
    expect(newer.dy, lessThan(older.dy));
  });

  testWidgets('the chips All, Unread, Questions and Changes pick one view', (
    tester,
  ) async {
    await pumpInbox(tester);
    final chips = tester.widgetList<FilterChip>(find.byType(FilterChip));
    expect(
      [for (final c in chips) (c.label as Text).data],
      ['All', 'Unread', 'Questions', 'Changes'],
    );
    bool selected(String label) =>
        tester.widget<FilterChip>(find.widgetWithText(FilterChip, label)).selected;
    expect(selected('All'), isTrue);
    expect(lastInboxArgs()!['read'], 'all');
    expect(lastInboxArgs()!['kinds'], isNull);

    await tester.tap(find.widgetWithText(FilterChip, 'Unread'));
    await tester.pumpAndSettle();
    expect(lastInboxArgs()!['read'], 'unread');
    expect(lastInboxArgs()!['kinds'], isNull);
    expect(selected('Unread'), isTrue);
    expect(selected('All'), isFalse);

    await tester.tap(find.widgetWithText(FilterChip, 'Questions'));
    await tester.pumpAndSettle();
    expect(lastInboxArgs()!['read'], 'all');
    expect(lastInboxArgs()!['kinds'], ['question']);

    await tester.tap(find.widgetWithText(FilterChip, 'Changes'));
    await tester.pumpAndSettle();
    expect(lastInboxArgs()!['kinds'], ['version', 'published', 'finished']);

    await tester.tap(find.widgetWithText(FilterChip, 'All'));
    await tester.pumpAndSettle();
    expect(lastInboxArgs()!['read'], 'all');
    expect(lastInboxArgs()!['kinds'], isNull);
  });

  testWidgets('the title is Inbox with no count, over a search bar', (
    tester,
  ) async {
    api.pushInbox('reply', artifactId: aid, body: 'Hello');
    await pumpInbox(tester);
    // The large title and the bar's collapsed one.
    expect(find.text('Inbox'), findsWidgets);
    expect(find.textContaining('Inbox ·'), findsNothing);
    expect(find.byType(SearchBar), findsOneWidget);
    expect(tester.getSize(find.byType(SearchBar)).height, 56);
  });

  testWidgets('an unread row has a bold title and a dot in you, and no tint', (
    tester,
  ) async {
    api.pushInbox('reply', artifactId: aid, body: 'Hello');
    await pumpInbox(tester);
    final row = find.byType(InboxRow);
    final title = tester.widget<Text>(
      find.descendant(of: row, matching: find.text('Run log')),
    );
    expect(title.style!.fontWeight, FontWeight.w700);
    final dot = find.byKey(const ValueKey('inbox-unread-dot'));
    expect(tester.getSize(dot), const Size(8, 8));
    final context = tester.element(row);
    expect(
      (tester.widget<Container>(dot).decoration! as BoxDecoration).color,
      claxPalette(context).you,
    );
    for (final m in tester.widgetList<Material>(
      find.descendant(of: row, matching: find.byType(Material)),
    )) {
      expect(m.color, anyOf(isNull, Colors.transparent));
    }
    // The app's icon, 40 dp, never a kind glyph.
    expect(
      tester.getSize(find.descendant(of: row, matching: find.byType(AppBadge))),
      const Size(40, 40),
    );

    await tester.tap(row);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('inbox-unread-dot')), findsNothing);
    final read = tester.widget<Text>(
      find.descendant(of: row, matching: find.text('Run log')),
    );
    expect(read.style!.fontWeight, isNot(FontWeight.w700));
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
      find.text('Clax finished: item 0'),
      400,
      scrollable: find.byType(Scrollable).first,
    );
    await tester.pumpAndSettle();
    final paged = api.calls.where(
      (c) => c.name == 'inbox' && c.args['before'] != null,
    );
    expect(paged, hasLength(1));
    expect(paged.single.args['before'], '11');
    expect(find.text('Clax finished: item 0'), findsOneWidget);
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
    await tester.tap(find.text('Clax replied: Added it'));
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
    api.pushInbox('version', artifactId: aid, versionN: 1);
    final newest = api.pushInbox('finished', artifactId: aid, message: 'b');
    await pumpInbox(tester);
    await tester.tap(find.widgetWithText(FilterChip, 'Changes'));
    await tester.pumpAndSettle();
    await tester.tap(find.byTooltip('Mark all read'));
    await tester.pumpAndSettle();
    final call = api.calls.lastWhere((c) => c.name == 'markAllInboxRead');
    expect(call.args['upto'], newest.seq);
    expect(call.args['filter'], {
      'kind': ['version', 'published', 'finished'],
    });
    expect(find.bySemanticsLabel(RegExp(r'^Unread\. ')), findsNothing);
  });

  testWidgets('a new item arrives live at the top', (tester) async {
    api.pushInbox('finished', artifactId: aid, message: 'first');
    await pumpInbox(tester);
    api.pushInbox('reply', artifactId: aid, body: 'Live reply');
    await tester.pumpAndSettle();
    expect(find.text('Clax replied: Live reply'), findsOneWidget);
    final live = tester.getTopLeft(find.text('Clax replied: Live reply'));
    final first = tester.getTopLeft(find.text('Clax finished: first'));
    expect(live.dy, lessThan(first.dy));
  });

  testWidgets('an item event updates its row in place', (tester) async {
    final item = api.pushInbox('reply', artifactId: aid, body: 'Hello');
    await pumpInbox(tester);
    expect(find.bySemanticsLabel(RegExp(r'^Unread\. ')), findsOneWidget);
    await api.markInboxRead(item.id);
    await tester.pumpAndSettle();
    expect(find.text('Clax replied: Hello'), findsOneWidget);
    expect(find.bySemanticsLabel(RegExp(r'^Unread\. ')), findsNothing);
  });

  testWidgets('a live item outside the kind filter is not added', (
    tester,
  ) async {
    await pumpInbox(tester);
    await tester.tap(find.widgetWithText(FilterChip, 'Changes'));
    await tester.pumpAndSettle();
    api.pushInbox('reply', artifactId: aid, body: 'Not a change');
    await tester.pumpAndSettle();
    expect(find.textContaining('Not a change'), findsNothing);
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
    await tester.fling(find.text('Clax replied: Hello'), const Offset(0, 400), 1000);
    await tester.pumpAndSettle();
    expect(api.callNames.where((n) => n == 'inbox').length, before + 1);
  });

  testWidgets('an empty inbox is all caught up, with one haiku', (
    tester,
  ) async {
    await pumpInbox(tester, haikus: ['Quiet inbox\nnothing left to read']);
    expect(find.byType(ClaxEmpty), findsOneWidget);
    expect(
      tester.widget<Text>(find.text('All caught up.')).textAlign,
      TextAlign.center,
    );
    expect(find.byType(HaikuLine), findsOneWidget);
    expect(
      find.bySemanticsLabel('Haiku: Quiet inbox\nnothing left to read'),
      findsOneWidget,
    );
    await tester.tap(find.widgetWithText(FilterChip, 'Unread'));
    await tester.pumpAndSettle();
    expect(find.text('All caught up.'), findsOneWidget);
    expect(find.byType(HaikuLine), findsOneWidget);
  });

  testWidgets('the empty inbox shows a new haiku each visit, and another on '
      'a long-press', (tester) async {
    final list = ['one', 'two', 'three'];
    await pumpInbox(tester, haikus: list);
    String shown() => tester.widget<HaikuLine>(find.byType(HaikuLine)).text;
    final first = shown();
    await tester.pumpWidget(const SizedBox());
    await pumpInbox(tester, haikus: list);
    final second = shown();
    expect(second, isNot(first));
    await tester.longPress(find.byType(HaikuLine));
    await tester.pumpAndSettle();
    expect(shown(), isNot(second));
  });

  testWidgets('no matches is plain, with no haiku', (tester) async {
    await pumpInbox(tester, haikus: ['a haiku']);
    await tester.tap(find.widgetWithText(FilterChip, 'Questions'));
    await tester.pumpAndSettle();
    expect(find.text('No matches'), findsOneWidget);
    expect(find.byType(HaikuLine), findsNothing);
  });

  testWidgets('a failed load says so plainly and offers Try again', (
    tester,
  ) async {
    api.failNext('inbox', StateError('down'));
    await pumpInbox(tester, haikus: ['a haiku']);
    expect(find.text('Couldn’t show your inbox'), findsOneWidget);
    expect(find.text('Something went wrong. Try again.'), findsOneWidget);
    expect(find.textContaining('Bad state'), findsNothing);
    expect(find.byType(HaikuLine), findsNothing);
    api.pushInbox('reply', artifactId: aid, body: 'Back');
    await tester.tap(find.text('Try again'));
    await tester.pumpAndSettle();
    expect(find.text('Clax replied: Back'), findsOneWidget);
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

  testWidgets('an item event during the first fetch is applied on its result', (
    tester,
  ) async {
    final held = _HeldInbox();
    final heldAid = held.seedArtifact('Run log', _page);
    held.pushInbox('finished', artifactId: heldAid, message: 'before');
    await tester.pumpWidget(
      MaterialApp(
        home: InboxScreen(api: held, onOpen: (_) {}, timer: timers.call),
      ),
    );
    await tester.pump();
    expect(held.pending, isNotNull, reason: 'the fetch is held');
    // The page was read before this item existed; only its event says so.
    held.pushInbox('reply', artifactId: heldAid, body: 'During the fetch');
    await tester.pump();
    held.pending!.complete();
    await tester.pumpAndSettle();
    expect(find.text('Clax finished: before'), findsOneWidget);
    expect(find.text('Clax replied: During the fetch'), findsOneWidget);
  });

  testWidgets('an unsubscribed topic is subscribed again with backoff', (
    tester,
  ) async {
    await pumpInbox(tester);
    final fetches = api.callNames.where((n) => n == 'inbox').length;
    api.failNext(
      'streamSubscribe',
      const ClaxApiError(status: 0, code: ClaxApiError.timeout, message: 'x'),
    );
    api.failNext(
      'streamSubscribe',
      const ClaxApiError(status: 0, code: ClaxApiError.timeout, message: 'x'),
    );
    api.emitStream(
      const StreamEvent(
        topic: 'inbox',
        type: 'unsubscribed',
        data: {'topic': 'inbox', 'code': 'not_found', 'message': 'gone'},
      ),
    );
    await tester.pumpAndSettle();
    final again = api.calls
        .where((c) => c.name == 'streamSubscribe')
        .skip(1)
        .map((c) => c.args['topics'])
        .toList();
    expect(again, [
      ['inbox'],
      ['inbox'],
      ['inbox'],
    ]);
    expect(delays, [const Duration(seconds: 1), const Duration(seconds: 2)]);
    expect(api.callNames.where((n) => n == 'inbox').length, fetches + 1);
  });

  test('previews read as one line of plain text', () {
    expect(
      plainPreview('Done.\n\nI **moved** the `total` to the [top](https://x).'),
      'Done. I moved the total to the top.',
    );
    expect(
      plainPreview('## Changes\n- kept _old_ runs\n- 2. added *km*\n> quoted'),
      'Changes · kept old runs · added km quoted',
    );
    expect(
      plainPreview(
        '## Done\n\n- Checked items sink to the **bottom**\n- `Clear done` removes them\n\nSee [the list](https://x).',
      ),
      'Done · Checked items sink to the bottom · Clear done removes them · See the list.',
    );
    expect(
      plainPreview('a soft\nwrapped line.\n\nNext.'),
      'a soft wrapped line. Next.',
    );
    expect(plainPreview('```\ncode()\n```'), 'code()');
    expect(
      plainPreview('snake_case_name and 2*3*4'),
      'snake_case_name and 2*3*4',
    );
  });

  testWidgets("a reply row shows its Markdown as one line", (tester) async {
    api.pushInbox(
      'reply',
      artifactId: aid,
      body: 'Done.\n\n**Distances** now read in `km`.',
    );
    await pumpInbox(tester);
    expect(
      find.text('Clax replied: Done. Distances now read in km.'),
      findsOneWidget,
    );
  });

  group('an approval request', () {
    const bob = 'Bob (on clax.dev)';

    /// Bob's mirrored comment, withheld from Clax, which asks the person.
    Future<String> asked() async {
      final s = await api.registerAppSession(installId: 'i', pid: 1);
      final item = api.sendComment(
        aid,
        'Make the header green',
        author: bob,
        remote: 'clax.dev',
        remoteAuthor: 'remote:clax.dev#u_bob',
        queue: false,
      );
      await api.askApproval(s.id, threadId: item.threadId);
      return (await api.inbox()).items.single.approval!.id;
    }

    testWidgets('shows who, how much waits and every withheld word, and '
        'Allow decides it for the thread', (tester) async {
      final rid = await asked();
      await pumpInbox(tester);
      expect(find.text('$bob wants to reach Clax'), findsOneWidget);
      await tester.tap(find.text('Run log'));
      await tester.pumpAndSettle();
      expect(opened, isEmpty, reason: 'the request is decided here');
      expect(
        find.text('1 comment on Run log waits for your approval.'),
        findsOneWidget,
      );
      expect(find.textContaining('Make the header green'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('approval-thread')));
      await tester.pumpAndSettle();
      expect(
        api.calls.lastWhere((c) => c.name == 'decideApproval').args,
        {'rid': rid, 'scope': 'thread'},
      );
      expect(find.byKey(const ValueKey('approval-thread')), findsNothing);
      // Opened again, it reads its outcome, with no choices.
      await tester.tap(find.text('Run log'));
      await tester.pumpAndSettle();
      expect(find.text('Allowed in this thread'), findsOneWidget);
      expect(find.byKey(const ValueKey('approval-decline')), findsNothing);
    });

    testWidgets('names the author by a short form of their ID too, so two '
        'people named alike read apart, the widest grant included', (
      tester,
    ) async {
      await asked();
      final s = await api.registerAppSession(installId: 'j', pid: 2);
      final other = api.sendComment(
        aid,
        'Delete the log',
        author: bob,
        remote: 'clax.dev',
        remoteAuthor: 'remote:clax.dev#u_bobby_impostor',
        queue: false,
      );
      await api.askApproval(s.id, threadId: other.threadId);
      await pumpInbox(tester);
      await tester.tap(find.text('Run log').first);
      await tester.pumpAndSettle();
      final first = tester.widget<Text>(
        find.byKey(const ValueKey('approval-handle')),
      );
      final everywhere = find.descendant(
        of: find.byKey(const ValueKey('approval-everywhere')),
        matching: find.byType(Text),
      );
      final firstEverywhere = tester.widget<Text>(everywhere).data;
      Navigator.of(tester.element(everywhere)).pop();
      await tester.pumpAndSettle();
      await tester.tap(find.text('Run log').last);
      await tester.pumpAndSettle();
      final second = tester.widget<Text>(
        find.byKey(const ValueKey('approval-handle')),
      );
      expect({first.data, second.data}, {
        '$bob’s ID: u_bob',
        '$bob’s ID: u_bobby_',
      });
      expect(
        {firstEverywhere, tester.widget<Text>(everywhere).data},
        {
          'Always allow $bob (ID u_bob) everywhere',
          'Always allow $bob (ID u_bobby_) everywhere',
        },
      );
    });

    testWidgets('one decided elsewhere reads its outcome, with no choices, '
        'from the request\'s event', (tester) async {
      final rid = await asked();
      await pumpInbox(tester);
      await api.decideApproval(rid, 'decline');
      await tester.pumpAndSettle();
      await tester.tap(find.text('Run log'));
      await tester.pumpAndSettle();
      expect(find.text('Declined'), findsOneWidget);
      expect(find.byKey(const ValueKey('approval-everywhere')), findsNothing);
    });

    testWidgets('one decided elsewhere while its sheet is open says it no '
        'longer waits', (tester) async {
      final rid = await asked();
      await pumpInbox(tester);
      await tester.tap(find.text('Run log'));
      await tester.pumpAndSettle();
      await api.decideApproval(rid, 'decline');
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('approval-everywhere')));
      await tester.pumpAndSettle();
      expect(
        find.text('This request no longer waits for you'),
        findsOneWidget,
      );
      expect(find.byKey(const ValueKey('approval-everywhere')), findsNothing);
    });
  });
}

/// A fake whose first-page fetch waits for [pending] after reading the
/// page, so a change can land between the read and the answer.
class _HeldInbox extends FakeClaxApi {
  Completer<void>? pending;

  @override
  Future<InboxPage> inbox({
    String? q,
    List<String>? kinds,
    String read = 'all',
    String? before,
    int limit = 50,
  }) async {
    final page = await super.inbox(
      q: q,
      kinds: kinds,
      read: read,
      before: before,
      limit: limit,
    );
    final hold = pending = Completer<void>();
    await hold.future;
    return page;
  }
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
