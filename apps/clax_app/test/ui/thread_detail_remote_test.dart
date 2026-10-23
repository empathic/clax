import 'dart:async';

import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/clax/models.dart';
import 'package:clax_app/platform/app_notice.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/remote/models.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/comment_composer.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/thread_detail.dart';
import 'package:clax_app/ui/threads_panel.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_surface.dart';
import '../support/fake_voice.dart';

RemoteHead _head({required bool comments}) => RemoteHead(
  artifactId: 'r1',
  url: 'https://clax.dev/a/r1',
  title: 'Runs',
  version: 3,
  contentSha256: 'sha256:${'0' * 64}',
  publishedBy: const PublishedBy(name: 'Ada'),
  owner: const PublishedBy(name: 'Ada'),
  comments: comments,
);

void main() {
  group('remote_comment_shows_source_tag', () {
    test('the core marks a comment from a remote in its linked', () {
      Map<String, Object?> comment(Object? linked) => {
        'id': 'c1',
        'author_kind': 'viewer',
        'author_name': 'Ada',
        'body': 'make it blue',
        'created_at': '2026-10-12T09:14:00Z',
        'linked': linked,
      };
      expect(
        ThreadComment.fromJson(
          comment({'origin': 'remote', 'remote': 'clax.dev'}),
        ).remoteOrigin,
        'clax.dev',
      );
      expect(
        ThreadComment.fromJson(
          comment({'origin': 'local', 'remote': 'clax.dev'}),
        ).remoteOrigin,
        isNull,
        reason: 'written here, sent up',
      );
      expect(ThreadComment.fromJson(comment(null)).remoteOrigin, isNull);
      expect(
        ThreadSummary.fromJson({
          'id': 't1',
          'status': 'open',
          'summary': 'make it blue',
          'sent': true,
          'comments': 1,
          'version': 1,
          'remote': 'clax.dev',
        }).remote,
        'clax.dev',
      );
    });

    testWidgets('beside the author on the thread page', (tester) async {
      final api = FakeClaxApi(clock: () => '2026-10-12T09:14:00Z');
      final aid = api.seedArtifact('Runs', {'index.html': '<h1>Runs</h1>'});
      await api.registerAppSession(installId: 'install', pid: 1);
      final remote = api.sendComment(
        aid,
        'make it blue',
        author: 'Ada',
        remote: 'clax.dev',
      );
      final here = api.sendComment(aid, 'and bigger', author: 'Bo');
      Future<void> pump(String tid) => tester.pumpWidget(
        MaterialApp(
          theme: claxTheme(Brightness.light, defaultTargetPlatform),
          home: ThreadDetail(
            key: ValueKey(tid),
            api: api,
            aid: aid,
            tid: tid,
            voice: FakeVoice(),
            autoDictate: false,
            onPreview: (_) {},
            onRetry: null,
            now: () => DateTime.utc(2026, 10, 12, 9, 20),
          ),
        ),
      );

      await pump(remote.threadId);
      await tester.pumpAndSettle();
      final tag = find.byKey(ValueKey('comment-source-${remote.commentId}'));
      expect(tag, findsOneWidget);
      expect(
        find.descendant(of: tag, matching: find.text('clax.dev')),
        findsOneWidget,
      );
      expect(find.text('Ada'), findsOneWidget);

      await pump(here.threadId);
      await tester.pumpAndSettle();
      expect(find.text('clax.dev'), findsNothing);
    });

    testWidgets('in the thread row', (tester) async {
      ThreadSummary row(String id, {String? remote}) => ThreadSummary(
        id: id,
        status: 'open',
        summary: 'make it blue $id',
        sent: true,
        comments: 1,
        version: 1,
        remote: remote,
      );
      await tester.pumpWidget(
        MaterialApp(
          theme: claxTheme(Brightness.light, defaultTargetPlatform),
          home: Scaffold(
            body: AppPanel(
              wide: true,
              tab: PanelTab.threads,
              onTab: (_) {},
              onClose: () {},
              threads: [row('t1', remote: 'clax.dev'), row('t2')],
              onThread: (_) {},
              onResend: (_) {},
              versions: const SizedBox(),
            ),
          ),
        ),
      );
      expect(find.text('clax.dev · 1 comment · sent to Clax'), findsOneWidget);
      expect(find.text('1 comment · sent to Clax'), findsOneWidget);
    });
  });

  group('composer_hidden_when_remote_comments_off', () {
    Future<(FakeAppSurface, StreamController<AppAction>)> pumpScreen(
      WidgetTester tester, {
      required bool? comments,
      String? initialThread,
      required FakeClaxApi api,
      required String aid,
    }) async {
      tester.view.devicePixelRatio = 3;
      tester.view.physicalSize = const Size(360, 780) * 3;
      addTearDown(tester.view.reset);
      final surface = FakeAppSurface();
      final actions = StreamController<AppAction>.broadcast(sync: true);
      addTearDown(actions.close);
      final worker = StreamController<WorkerEvent>.broadcast(sync: true);
      addTearDown(worker.close);
      await tester.pumpWidget(
        MaterialApp(
          theme: claxTheme(Brightness.light, defaultTargetPlatform),
          home: AppScreen(
            aid: aid,
            base: Uri.parse('http://127.0.0.1:7999'),
            api: api,
            surface: surface,
            voice: FakeVoice(),
            settings: AppSettings(),
            timings: const NoTimings(),
            worker: worker.stream,
            actions: actions.stream,
            initialThread: initialThread,
            linkedHead: comments == null
                ? null
                : (a) async => a == aid ? _head(comments: comments) : null,
          ),
        ),
      );
      await tester.pump();
      surface.push(
        Ready(artifact: aid, version: 1, latest: 1, file: 'index.html'),
      );
      await tester.pump();
      await tester.pump(const Duration(seconds: 1));
      return (surface, actions);
    }

    const draft = StateMsg(
      commenting: true,
      draft: Draft(label: 'h1', text: '', clip: DraftClip.pending),
      busy: false,
    );

    for (final comments in [false, true, null]) {
      final off = comments == false;
      testWidgets(
        off
            ? 'comments off: no Comment, no comment mode, no composer'
            : 'comments ${comments == null ? 'unknown' : 'on'}: the composer as ever',
        (tester) async {
          final api = FakeClaxApi();
          final aid = api.seedArtifact('Runs', {'index.html': '<h1>R</h1>'});
          await api.registerAppSession(installId: 'install', pid: 1);
          final (surface, actions) = await pumpScreen(
            tester,
            comments: comments,
            api: api,
            aid: aid,
          );
          int enters() => surface.hostCalls.whereType<EnterCommentMode>().length;

          actions.add(AppAction.comment);
          await tester.pump();
          actions.add(AppAction.threads);
          await tester.pumpAndSettle();
          expect(enters(), off ? 0 : greaterThan(0));

          surface.push(draft);
          await tester.pumpAndSettle();
          expect(
            find.byType(CommentComposer),
            off ? findsNothing : findsOneWidget,
          );
        },
      );
    }

    for (final comments in [false, true]) {
      testWidgets(
        'comments ${comments ? 'on' : 'off'}: '
        'a thread page ${comments ? 'has' : 'has no'} reply bar',
        (tester) async {
          final api = FakeClaxApi();
          final aid = api.seedArtifact('Runs', {'index.html': '<h1>R</h1>'});
          await api.registerAppSession(installId: 'install', pid: 1);
          final t = api.sendComment(aid, 'make it blue', remote: 'clax.dev');
          await pumpScreen(
            tester,
            comments: comments,
            api: api,
            aid: aid,
            initialThread: t.threadId,
          );
          await tester.pumpAndSettle();
          expect(find.byType(ThreadDetail), findsOneWidget);
          expect(
            find.byKey(const ValueKey('thread-reply-bar')),
            comments ? findsOneWidget : findsNothing,
          );
          expect(
            find.text('Comments are off on clax.dev. Turn them on in Share….'),
            comments ? findsNothing : findsOneWidget,
          );
        },
      );
    }
  });

  testWidgets('a thread whose comments wait on the person\'s approval says '
      'so, and Review… decides it there', (tester) async {
    const bob = 'Bob (on clax.dev)';
    final api = FakeClaxApi(clock: () => '2026-10-12T09:14:00Z');
    final aid = api.seedArtifact('Runs', {'index.html': '<h1>Runs</h1>'});
    final s = await api.registerAppSession(installId: 'install', pid: 1);
    final t = api.sendComment(
      aid,
      'make it blue',
      author: bob,
      remote: 'clax.dev',
      remoteAuthor: 'remote:clax.dev#u_bob',
      queue: false,
    );
    await api.askApproval(s.id, threadId: t.threadId);
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.light, defaultTargetPlatform),
        home: ThreadDetail(
          api: api,
          aid: aid,
          tid: t.threadId,
          voice: FakeVoice(),
          autoDictate: false,
          onPreview: (_) {},
          onRetry: null,
          now: () => DateTime.utc(2026, 10, 12, 9, 20),
        ),
      ),
    );
    await tester.pumpAndSettle();
    const line = 'Clax keeps $bob’s comments from Claude until you allow them.';
    expect(find.text(line), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('thread-approval-review')));
    await tester.pumpAndSettle();
    expect(find.text('$bob wants to reach Clax'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('approval-thread')));
    await tester.pumpAndSettle();
    expect(
      api.calls.lastWhere((c) => c.name == 'decideApproval').args['scope'],
      'thread',
    );
    expect(find.text(line), findsNothing);
  });
}
