import 'dart:async';

import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/clax/models.dart';
import 'package:clax_app/platform/app_notice.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/remote/checker.dart';
import 'package:clax_app/remote/models.dart';
import 'package:clax_app/remote/sync_state.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/comment_composer.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/remote/linked_app.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/thread_detail.dart';
import 'package:clax_app/ui/working_chip.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_surface.dart';
import '../support/fake_voice.dart';

RemoteHead _head({
  bool comments = true,
  int version = 1,
  RemoteBundle? bundle,
}) => RemoteHead(
  artifactId: 'r1',
  url: 'https://clax.dev/a/r1',
  title: 'Runs',
  version: version,
  contentSha256: 'sha256:${'0' * 64}',
  publishedBy: const PublishedBy(name: 'Ada', login: 'ada'),
  owner: const PublishedBy(name: 'Ada', login: 'ada'),
  comments: comments,
  bundle: bundle,
);

LineageEntry _entry({String role = AppLink.author, List<int> published = const [1]}) =>
    LineageEntry(
      lineage: 'l_aaaaaaaaaaaaaaaaaaaaaaaaaa',
      role: LineageRole.author,
      key: 'k',
      name: null,
      version: 1,
      released: const [],
      capabilities: const {},
      title: 'Runs',
      link: AppLink(
        remoteOrigin: 'https://clax.dev',
        remoteId: 'r1',
        remoteArtifactId: 'r1',
        url: 'https://clax.dev/a/r1',
        baseRemoteVersion: published.length,
        role: role,
        published: published,
      ),
    );

class _Rig {
  _Rig(this.api, this.aid, this.surface, this.actions, this.worker);

  final FakeClaxApi api;
  final String aid;
  final FakeAppSurface surface;
  final StreamController<AppAction> actions;
  final StreamController<WorkerEvent> worker;
  final status = ValueNotifier<Map<String, LinkedStatus>>(const {});
  final published = <int>[];
  var updates = 0;
  var resolves = 0;
  var opened = <String>[];
  LineageEntry? entry = _entry();

  void set(LinkedStatus s) => status.value = {aid: s};

  LinkedAppHooks hooks() => LinkedAppHooks(
    status: status,
    lineage: (_) async => entry,
    opened: (aid) async => opened.add(aid),
    publish: (context, aid, n) async => published.add(n),
    update: (context, aid) async => updates++,
    diverged: (context, aid) async => resolves++,
  );
}

void main() {
  Future<_Rig> pumpScreen(
    WidgetTester tester, {
    void Function(FakeClaxApi api, String aid)? seed,
  }) async {
    tester.view.devicePixelRatio = 3;
    tester.view.physicalSize = const Size(360, 1400) * 3;
    addTearDown(tester.view.reset);
    final api = FakeClaxApi();
    final aid = api.seedArtifact('Runs', {'index.html': '<h1>R</h1>'});
    await api.registerAppSession(installId: 'install', pid: 1);
    seed?.call(api, aid);
    final surface = FakeAppSurface();
    final actions = StreamController<AppAction>.broadcast(sync: true);
    addTearDown(actions.close);
    final worker = StreamController<WorkerEvent>.broadcast(sync: true);
    addTearDown(worker.close);
    final rig = _Rig(api, aid, surface, actions, worker);
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
          linked: rig.hooks(),
        ),
      ),
    );
    await tester.pump();
    surface.push(
      Ready(artifact: aid, version: 1, latest: 1, file: 'index.html'),
    );
    await tester.pump();
    await tester.pump(const Duration(seconds: 1));
    return rig;
  }

  Finder pill() => find.byKey(const ValueKey('clax-pill'));

  testWidgets('ahead_pill_and_publish_toast_after_change', (tester) async {
    final rig = await pumpScreen(
      tester,
      seed: (api, aid) => unawaited(
        api.publish(
          aid,
          PublishRequest(
            ifVersion: 1,
            files: {'index.html': '<h1>R2</h1>'},
            note: 'Made the Save button blue',
          ),
        ),
      ),
    );
    expect(rig.opened, [rig.aid], reason: 'the checker hears it opened');
    rig.set(LinkedStatus(state: SyncState.live, head: _head(), localVersion: 1, publicVersion: 1));
    await tester.pump();
    expect(pill(), findsNothing, reason: 'live says nothing on the pill');

    // A change lands as v2, the page showing it already.
    rig.surface.push(
      Ready(artifact: rig.aid, version: 2, latest: 2, file: 'index.html'),
    );
    await tester.pump();
    rig.worker.add(WorkerLanded(rig.aid, 2));
    await tester.pump();
    expect(find.textContaining('Claude made'), findsNothing,
        reason: 'not before the checker reads it ahead');

    rig.set(LinkedStatus(state: SyncState.ahead, head: _head(), localVersion: 2, publicVersion: 1));
    await tester.pump();
    await tester.pump();
    expect(find.text('Claude made v2: Made the Save button blue'), findsOneWidget);
    await tester.tap(find.text('Publish'));
    await tester.pump();
    expect(rig.published, [2]);

    // The toast gone, the pill says where the app stands; a tap publishes.
    await tester.pump(const Duration(seconds: 10));
    await tester.pumpAndSettle();
    expect(find.text('↑ v2 here · v1 public'), findsNothing,
        reason: 'folded after a while');
    rig.set(LinkedStatus(state: SyncState.ahead, head: _head(), localVersion: 3, publicVersion: 1));
    await tester.pump();
    expect(find.text('↑ v3 here · v1 public'), findsOneWidget);
    expect(find.textContaining('Claude made v3'), findsNothing,
        reason: 'only a change that landed here is offered by toast');
    await tester.tap(find.byKey(const ValueKey('clax-pill-target')));
    await tester.pump();
    expect(rig.published, [2, 3]);

    // Published: live again, and the pill goes.
    rig.set(LinkedStatus(state: SyncState.live, head: _head(version: 2), localVersion: 3, publicVersion: 3));
    await tester.pumpAndSettle();
    expect(pill(), findsNothing);
    await tester.pump(const Duration(seconds: 10));
  });

  testWidgets('a waiting update shows on the pill and in a toast, whose '
      'Update offers it', (tester) async {
    final rig = await pumpScreen(tester);
    rig.set(
      LinkedStatus(
        state: SyncState.updateWaiting,
        head: _head(
          version: 2,
          bundle: const RemoteBundle(
            version: 2,
            lineage: 'l_aaaaaaaaaaaaaaaaaaaaaaaaaa',
            bundleVersion: 6,
            contentSha256: 'sha256:x',
            url: 'https://clax.dev/b',
            publishedBy: PublishedBy(name: 'Ed', login: 'ed'),
          ),
        ),
        localVersion: 1,
      ),
    );
    await tester.pump();
    await tester.pump();
    expect(find.text('v6 is ready · from @ed'), findsOneWidget);
    await tester.tap(find.text('Update'));
    await tester.pump();
    expect(rig.updates, 1);
    await tester.pump(const Duration(seconds: 10));
    await tester.pumpAndSettle();
    expect(pill(), findsOneWidget);
    expect(
      tester.widget<ClaxPill>(find.byType(ClaxPill)).state,
      ClaxPillState.update,
    );
    await tester.tap(find.byKey(const ValueKey('clax-pill-target')));
    await tester.pump();
    expect(rig.updates, 2);
    await tester.pump(const Duration(seconds: 10));
  });

  testWidgets('a diverged app shows Diverged on the pill, whose tap '
      'offers the diverged choices', (tester) async {
    final rig = await pumpScreen(tester);
    rig.set(
      LinkedStatus(
        state: SyncState.diverged,
        head: _head(version: 3),
        localVersion: 4,
        publicVersion: 2,
      ),
    );
    await tester.pump();
    await tester.pump();
    await tester.pumpAndSettle();
    expect(
      tester.widget<ClaxPill>(find.byType(ClaxPill)).state,
      ClaxPillState.diverged,
    );
    expect(find.text('Diverged'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('clax-pill-target')));
    await tester.pump();
    expect(rig.resolves, 1);
    await tester.pump(const Duration(seconds: 10));
  });

  testWidgets('the composer goes when the checker’s head says comments are '
      'off, and comes back when they are on', (tester) async {
    final rig = await pumpScreen(tester);
    int enters() => rig.surface.hostCalls.whereType<EnterCommentMode>().length;
    const draft = StateMsg(
      commenting: true,
      draft: Draft(label: 'h1', text: '', clip: DraftClip.pending),
      busy: false,
    );

    rig.set(LinkedStatus(state: SyncState.live, head: _head(comments: false)));
    await tester.pumpAndSettle();
    final before = enters();
    rig.actions.add(AppAction.comment);
    await tester.pumpAndSettle();
    expect(enters(), before, reason: 'no comment mode');
    rig.surface.push(draft);
    await tester.pumpAndSettle();
    expect(find.byType(CommentComposer), findsNothing);

    rig.set(LinkedStatus(state: SyncState.live, head: _head()));
    await tester.pumpAndSettle();
    rig.actions.add(AppAction.comment);
    await tester.pumpAndSettle();
    expect(enters(), greaterThan(before));
    rig.surface.push(draft);
    await tester.pumpAndSettle();
    expect(find.byType(CommentComposer), findsOneWidget);
  });

  testWidgets('Versions marks each version against the public copy, with '
      'Publish for an author', (tester) async {
    final rig = await pumpScreen(
      tester,
      seed: (api, aid) {
        unawaited(() async {
          await api.publish(
            aid,
            PublishRequest(ifVersion: 1, files: {'index.html': '2'}),
          );
          await api.publish(
            aid,
            PublishRequest(ifVersion: 2, files: {'index.html': '3'}),
          );
        }());
      },
    );
    rig.entry = _entry(published: [1, 2]);
    rig.set(LinkedStatus(state: SyncState.ahead, head: _head(version: 2), localVersion: 3, publicVersion: 2));
    await tester.pump();
    rig.actions.add(AppAction.threads);
    await tester.pumpAndSettle();
    await tester.tap(find.text('Versions'));
    await tester.pumpAndSettle();
    String mark(int n) =>
        tester.widget<Text>(find.byKey(ValueKey('mark-$n'))).semanticsLabel!;
    expect(mark(1), 'was public');
    expect(mark(2), 'public');
    expect(mark(3), 'here');
    expect(find.byKey(const ValueKey('publish-2')), findsNothing);
    await tester.ensureVisible(find.byKey(const ValueKey('publish-1')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('publish-1')));
    await tester.pump();
    expect(rig.published, [1]);
    await tester.pump(const Duration(seconds: 10));
  });

  testWidgets('a recipient’s Versions have marks but no Publish', (
    tester,
  ) async {
    final rig = await pumpScreen(tester);
    rig.entry = _entry(role: AppLink.recipient, published: const []);
    rig.set(LinkedStatus(state: SyncState.live, head: _head()));
    await tester.pump();
    rig.actions.add(AppAction.threads);
    await tester.pumpAndSettle();
    await tester.tap(find.text('Versions'));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('mark-1')), findsOneWidget);
    expect(find.byKey(const ValueKey('publish-1')), findsNothing);
  });

  testWidgets('a held reply whose background publish was refused says why', (
    tester,
  ) async {
    final api = FakeClaxApi(clock: () => '2026-10-12T09:14:00Z');
    final aid = api.seedArtifact('Runs', {'index.html': '<h1>Runs</h1>'});
    await api.registerAppSession(installId: 'install', pid: 1);
    final t = api.sendComment(aid, 'make it blue', remote: 'clax.dev');
    final noted = <int>[];
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
          heldReply: () async => 2,
          heldNote: (n) async {
            noted.add(n);
            return 'v2 wasn’t published: the public copy changed.';
          },
          publishAndReply: (n) async => null,
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(noted, [2]);
    expect(find.text('Claude’s reply waits for v2 to be public.'), findsOneWidget);
    expect(
      find.text('v2 wasn’t published: the public copy changed.'),
      findsOneWidget,
    );
  });

  testWidgets('a thread whose reply waits offers Publish vN and reply', (
    tester,
  ) async {
    final api = FakeClaxApi(clock: () => '2026-10-12T09:14:00Z');
    final aid = api.seedArtifact('Runs', {'index.html': '<h1>Runs</h1>'});
    await api.registerAppSession(installId: 'install', pid: 1);
    final t = api.sendComment(aid, 'make it blue', remote: 'clax.dev');
    int? held = 2;
    final asked = <int>[];
    String? answer = 'Couldn’t publish v2.';
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
          heldReply: () async => held,
          publishAndReply: (n) async {
            asked.add(n);
            if (answer == null) held = null;
            return answer;
          },
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Claude’s reply waits for v2 to be public.'), findsOneWidget);
    await tester.tap(find.text('Publish v2 and reply'));
    await tester.pumpAndSettle();
    expect(asked, [2]);
    expect(find.text('Couldn’t publish v2.'), findsOneWidget);

    answer = null;
    await tester.tap(find.text('Publish v2 and reply'));
    await tester.pumpAndSettle();
    expect(asked, [2, 2]);
    expect(find.byKey(const ValueKey('held-reply')), findsNothing);
  });
}
