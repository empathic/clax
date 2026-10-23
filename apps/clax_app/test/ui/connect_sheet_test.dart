import 'dart:async';

import 'package:clax_app/clax/api.dart' show ClaxApiError;
import 'package:clax_app/remote/fake_remote_api.dart';
import 'package:clax_app/remote/models.dart';
import 'package:clax_app/ui/remote/connect_sheet.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_clock.dart' show FakeClock;

/// A [FakeRemoteApi] whose polls, while [gate] is set, find their answer
/// at once but give it only once [gate] completes.
class _HeldPolls extends FakeRemoteApi {
  Completer<void>? gate;

  @override
  Future<Remote> poll(String rid) async {
    final answer = await super.poll(rid);
    if (gate case final g?) await g.future;
    return answer;
  }
}

void main() {
  late _HeldPolls remote;
  late FakeClock clock;
  late List<Uri> opened;
  late int closed;
  Remote? answer;
  var answered = false;
  var opens = true;

  setUp(() {
    remote = _HeldPolls()..signInsWait = true;
    clock = FakeClock();
    opened = [];
    closed = 0;
    answer = null;
    answered = false;
    opens = true;
  });

  Widget host() => MaterialApp(
    theme: claxTheme(Brightness.light, defaultTargetPlatform),
    home: Scaffold(
      appBar: AppBar(title: const Text('Settings')),
      body: Builder(
        builder: (context) => TextButton(
          onPressed: () async {
            answer = await showConnectSheet(
              context,
              remote,
              clock: clock,
              device: 'Pixel 9',
              openAuth: (u) async {
                opened.add(u);
                return opens;
              },
              closeAuth: () async => closed++,
            );
            answered = true;
          },
          child: const Text('go'),
        ),
      ),
    ),
  );

  /// Lets the fake's futures and the clock's delays run, then draws.
  Future<void> advance(WidgetTester tester, Duration d) async {
    await tester.runAsync(() => clock.advance(d));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));
  }

  Future<void> open(WidgetTester tester) async {
    await tester.pumpWidget(host());
    await tester.tap(find.text('go'));
    await advance(tester, Duration.zero);
  }

  testWidgets('connect_shows_code_and_connects_on_poll', (tester) async {
    await open(tester);
    expect(remote.signIns.single, (
      url: 'https://clax.dev',
      kind: 'clax-app',
      machine: 'Clax on Pixel 9',
    ));
    final rid = remote.remotesById.keys.single;
    final code = 'CODE-${rid.toUpperCase()}';
    expect(
      find.text(
        'clax.dev is open in your browser. Sign in there, then check that '
        'it shows this code:',
      ),
      findsOneWidget,
    );
    expect(find.text(code), findsOneWidget);
    expect(
      find.text(
        'This device will be listed as Clax on Pixel 9. You can disconnect '
        'it from either side at any time.',
      ),
      findsOneWidget,
    );
    expect(opened, [Uri.parse('https://clax.dev/device?code=$code')]);

    // Not yet approved: it keeps asking, every interval.
    await advance(tester, const Duration(seconds: 5));
    await advance(tester, const Duration(seconds: 5));
    expect(remote.calls.where((c) => c == 'poll'), hasLength(2));
    expect(answered, isFalse);

    await tester.tap(find.text('Open clax.dev again'));
    await advance(tester, Duration.zero);
    expect(opened, hasLength(2));

    remote.approve(rid);
    await advance(tester, const Duration(seconds: 5));
    expect(answered, isTrue);
    expect(answer?.connected, isTrue);
    expect(answer?.id, rid);
    expect(closed, 1, reason: 'the browser session over the app closes');
    expect(find.text(code), findsNothing);
  });

  testWidgets('connect_expired_offers_retry', (tester) async {
    await open(tester);
    final first = remote.remotesById.keys.single;
    await advance(tester, const Duration(minutes: 9, seconds: 55));
    expect(find.text('That code expired. Try again.'), findsNothing);
    await advance(tester, const Duration(seconds: 5));
    expect(find.text('That code expired. Try again.'), findsOneWidget);
    expect(find.text('CODE-${first.toUpperCase()}'), findsNothing);
    expect(
      remote.remotesById,
      isEmpty,
      reason: 'the lapsed sign-in is forgotten',
    );
    final polls = remote.calls.where((c) => c == 'poll').length;
    await advance(tester, const Duration(minutes: 1));
    expect(
      remote.calls.where((c) => c == 'poll'),
      hasLength(polls),
      reason: 'an expired code is not polled',
    );

    await tester.tap(find.text('Try again'));
    await advance(tester, Duration.zero);
    expect(remote.signIns, hasLength(2));
    final second = remote.remotesById.keys.single;
    expect(find.text('CODE-${second.toUpperCase()}'), findsOneWidget);
    remote.approve(second);
    await advance(tester, const Duration(seconds: 5));
    expect(answer?.id, second);
  });

  testWidgets('the remote saying the code expired offers retry too', (
    tester,
  ) async {
    await open(tester);
    remote.expire(remote.remotesById.keys.single);
    await advance(tester, const Duration(seconds: 5));
    expect(find.text('That code expired. Try again.'), findsOneWidget);
  });

  testWidgets('Cancel while a poll is out: the sheet closes once, and a '
      'sign-in the poll finishes is not kept', (tester) async {
    await open(tester);
    final rid = remote.remotesById.keys.single;
    remote
      ..gate = Completer<void>()
      ..approve(rid);
    await advance(tester, const Duration(seconds: 5));
    expect(remote.calls.where((c) => c == 'poll'), hasLength(1));
    expect(answered, isFalse);

    await tester.tap(find.text('Cancel'));
    await tester.pump();
    remote.gate!.complete();
    await advance(tester, Duration.zero);
    await advance(tester, const Duration(seconds: 1));

    expect(find.text('Settings'), findsOneWidget, reason: 'popped only once');
    expect(answered, isTrue);
    expect(answer, isNull);
    expect(remote.remotesById, isEmpty);
    expect(closed, 1);
  });

  testWidgets('signed in there already, the sheet answers that remote', (
    tester,
  ) async {
    final had = remote.connected('https://clax.dev');
    await open(tester);
    expect(answered, isTrue);
    expect(answer?.id, had.id);
    expect(find.text('Try again'), findsNothing);
  });

  test('the machine name fits the remote: at most 64 characters, no control '
      'characters', () {
    expect(machineName('Pixel 9'), 'Clax on Pixel 9');
    expect(machineName(' Sam\u0007’s\nMac '), 'Clax on Sam’sMac');
    final long = machineName('M' * 80);
    expect(long.runes.length, lessThanOrEqualTo(FakeRemoteApi.maxMachineChars));
    expect(long, endsWith('…'));
    expect(machineName('\u0085'), 'Clax on this device');
  });

  testWidgets('a long device name still signs in', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.light, defaultTargetPlatform),
        home: Scaffold(
          body: Builder(
            builder: (context) => TextButton(
              onPressed: () => showConnectSheet(
                context,
                remote,
                clock: clock,
                device: 'A very long Computer Name that someone typed ' * 3,
                openAuth: (_) async => true,
                closeAuth: () async {},
              ),
              child: const Text('go'),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('go'));
    await advance(tester, Duration.zero);
    expect(remote.signIns.single.machine!.runes.length, lessThanOrEqualTo(64));
    expect(find.textContaining('CODE-'), findsOneWidget);
  });

  testWidgets('Cancel forgets the waiting sign-in and answers null', (
    tester,
  ) async {
    await open(tester);
    await tester.tap(find.text('Cancel'));
    await advance(tester, Duration.zero);
    expect(answered, isTrue);
    expect(answer, isNull);
    expect(remote.remotesById, isEmpty);
    await advance(tester, const Duration(seconds: 30));
    expect(remote.calls.where((c) => c == 'poll'), isEmpty);
  });

  group('a sign-in the core refuses', () {
    int polls() => remote.calls.where((c) => c == 'poll').length;

    testWidgets('clax.dev too old for this app says it needs an update, with '
        'no Try again', (tester) async {
      remote.tooOld.add('https://clax.dev');
      await open(tester);
      expect(
        find.text('clax.dev needs an update before the Clax app can sign in.'),
        findsOneWidget,
      );
      expect(find.text('Try again'), findsNothing);
      expect(find.text('Close'), findsOneWidget);
    });

    testWidgets('a remote that refuses the start is told once, with no Try '
        'again', (tester) async {
      remote.failNext(
        'addRemote',
        const ClaxApiError(
          status: 502,
          code: 'remote_refused',
          message: 'clax.dev answered 400',
          body: {'remote_status': 400, 'remote_code': 'invalid_args'},
        ),
      );
      await open(tester);
      expect(
        find.text('clax.dev didn’t accept this device’s sign-in.'),
        findsOneWidget,
      );
      expect(find.text('Try again'), findsNothing);
    });

    testWidgets('a remote out of reach says so and offers Try again', (
      tester,
    ) async {
      remote.failNext(
        'addRemote',
        const ClaxApiError(
          status: 502,
          code: 'remote_unreachable',
          message: 'clax.dev could not be reached',
        ),
      );
      await open(tester);
      expect(
        find.text('Couldn’t reach clax.dev. Check your connection and try '
            'again.'),
        findsOneWidget,
      );
      await tester.tap(find.text('Try again'));
      await advance(tester, Duration.zero);
      expect(find.textContaining('CODE-'), findsOneWidget);
    });

    testWidgets('a poll the remote refuses stops at once, not when the code '
        'lapses', (tester) async {
      await open(tester);
      remote.failNext(
        'poll',
        const ClaxApiError(
          status: 502,
          code: 'remote_refused',
          message: 'clax.dev answered 404',
          body: {'remote_status': 404, 'remote_code': 'not_found'},
        ),
      );
      await advance(tester, const Duration(seconds: 5));
      expect(
        find.text('Couldn’t finish signing in to clax.dev. Try again.'),
        findsOneWidget,
      );
      expect(remote.remotesById, isEmpty, reason: 'the sign-in is forgotten');
      final n = polls();
      await advance(tester, const Duration(minutes: 1));
      expect(polls(), n, reason: 'a refused sign-in is not polled');
    });

    testWidgets('a poll the remote is too old for says it needs an update', (
      tester,
    ) async {
      await open(tester);
      remote.tooOld.add('https://clax.dev');
      await advance(tester, const Duration(seconds: 5));
      expect(
        find.text('clax.dev needs an update before the Clax app can sign in.'),
        findsOneWidget,
      );
      expect(find.text('Try again'), findsNothing);
    });

    testWidgets('a poll that finds the remote out of reach asks again', (
      tester,
    ) async {
      await open(tester);
      remote.failNext(
        'poll',
        const ClaxApiError(
          status: 502,
          code: 'remote_unreachable',
          message: 'clax.dev could not be reached',
        ),
      );
      await advance(tester, const Duration(seconds: 5));
      await advance(tester, const Duration(seconds: 5));
      expect(polls(), 2);
      expect(find.textContaining('CODE-'), findsOneWidget);
    });
  });

  testWidgets('a sign-in page that did not open shows its address to go to '
      'by hand', (tester) async {
    opens = false;
    await open(tester);
    final rid = remote.remotesById.keys.single;
    final url = 'https://clax.dev/device?code=CODE-${rid.toUpperCase()}';
    expect(find.textContaining('is open in your browser'), findsNothing);
    expect(
      find.text(
        'clax.dev didn’t open. Go to this address on any device and sign '
        'in, then check that it shows the code below:',
      ),
      findsOneWidget,
    );
    expect(find.text(url), findsOneWidget);
    expect(find.byKey(const ValueKey('connect-copy')), findsOneWidget);
    expect(find.text('Open clax.dev again'), findsOneWidget);

    // Opened on the next try: the usual line.
    opens = true;
    await tester.tap(find.text('Open clax.dev again'));
    await advance(tester, Duration.zero);
    expect(find.textContaining('is open in your browser'), findsOneWidget);
  });
}
