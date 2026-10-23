import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';

import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/agent/reply_hold.dart';
import 'package:clax_app/app.dart';
import 'package:clax_app/bundle/exporter.dart';
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/core/core_client.dart';
import 'package:clax_app/platform/bundle_key.dart';
import 'package:clax_app/platform/clock.dart';
import 'package:clax_app/platform/secure_key.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/remote/fake_remote_api.dart';
import 'package:clax_app/remote/publisher.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/remote/linked_app.dart';
import 'package:clax_app/ui/tile_label.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'agent/loop_support.dart';
import 'bundle/services_support.dart';
import 'support/fake_clock.dart' show FakeClock;
import 'support/fake_core.dart';
import 'support/fake_inspector.dart';
import 'support/fake_surface.dart';
import 'support/fake_voice.dart';
import 'support/settle.dart';
import 'ui/bundle_flows_test.dart' show FakeBundleFiles;

/// The app's held replies in a file, counting the writes asked for and
/// those done, so a test can wait for every write made so far.
class _Kept implements HeldReplies {
  _Kept(File file) : _inner = FileHeldReplies(file);

  final FileHeldReplies _inner;
  var asked = 0;
  var done = 0;

  bool get written => done == asked;

  @override
  Future<List<HeldReply>> read() => _inner.read();

  @override
  Future<void> write(List<HeldReply> replies) {
    asked++;
    return _inner.write(replies).whenComplete(() => done++);
  }
}

/// The launcher tile titled [title], found by its label.
Finder _tile(String title) =>
    find.byWidgetPredicate((w) => w is TileLabel && w.text == title);

class _Key implements SecureKey {
  @override
  Future<String?> read() async => 'sk-ant-x';

  @override
  Future<void> write(String? key) async {}
}

void main() {
  late Directory dir;
  late FakeClaxApi api;
  late FakeCoreHost coreHost;
  late FakeRemoteApi remote;
  late BundleKey key;
  late List<FakeAppSurface> surfaces;
  late _Kept kept;
  FakeClock? clock;
  late List<Object> script;

  setUp(() {
    dir = Directory.systemTemp.createTempSync('app-remote-');
    api = FakeClaxApi();
    coreHost = FakeCoreHost();
    remote = FakeRemoteApi();
    key = BundleKey(MemorySeed(), random: Random(3));
    surfaces = [];
    kept = _Kept(File('${dir.path}/held-replies.json'));
    clock = null;
    script = [];
  });
  tearDown(() => dir.deleteSync(recursive: true));

  AppServices services() => AppServices(
    core: coreHost,
    api: (_) => api,
    backend: (s, k) => HeldBackend(script, name: backendMessagesApi),
    voice: FakeVoice.unsupported('No speech here'),
    settings: SettingsStore(File('${dir.path}/settings.json')),
    key: _Key(),
    timings: RecordingTimings(),
    agentTimings: RecordingTimings(),
    surface: () {
      final s = FakeAppSurface();
      surfaces.add(s);
      return s;
    },
    installId: 'install',
    supportDir: dir.path,
    prompts: PromptBuilder(rootBundle),
    bundleKey: key,
    bundleFiles: FakeBundleFiles(),
    inspector: FakeInspector(),
    remote: () => remote,
    heldReplies: kept,
    clock: clock ?? const SystemClock(),
    pid: 42,
  );

  List<String> replies() => [
    for (final c in api.calls)
      if (c.name == 'replyAsAgent') '${c.args['tid']}: ${c.args['body']}',
  ];

  /// [aid]'s linked status as the app's checker holds it, read through the
  /// open app screen.
  LinkedAppHooks hooks(WidgetTester tester) =>
      tester.widget<AppScreen>(find.byType(AppScreen)).linked!;

  testWidgets('a reply held for v2 goes out once an ordinary publish makes '
      'v2 public', (tester) async {
    late String aid, tid;
    await tester.runAsync(() async {
      await api.registerAppSession(installId: 'install', pid: 42);
      aid = (await api.createArtifact(
        PublishRequest(title: 'Plant log', files: {'index.html': '<p>v1</p>'}),
      )).artifact.id;
      await api.publish(
        aid,
        PublishRequest(ifVersion: 1, files: {'index.html': '<p>v2</p>'}),
      );
      for (final v in (await api.getArtifact(aid)).versions) {
        remote.localVersion(aid, v.n, contentSha256: v.contentSha256!);
      }
      remote.connected('https://clax.dev');
      // v1 is public already, published from this device.
      final store = LineageStore(dir);
      final out = await Publisher(
        remote: remote,
        exporter: Exporter(api: api, store: store, key: key),
        store: store,
        clock: FakeClock(),
      ).publish(aid, version: 1);
      expect(out, isA<Published>());
      // Claude answered a thread from clax.dev with v2, held until v2 is
      // public.
      tid = api
          .sendComment(aid, 'make it blue', remote: 'clax.dev', queue: false)
          .threadId;
      File('${dir.path}/held-replies.json').writeAsStringSync(
        jsonEncode({
          'version': 1,
          'replies': [
            {
              'artifact_id': aid,
              'thread_id': tid,
              'text': 'Made it blue.',
              'version': 2,
              'resolve': false,
            },
          ],
        }),
      );
    });
    await tester.pumpWidget(AppRoot(services: services()));
    coreHost.set(const CoreReady());
    await settle(tester, _tile('Plant log'));
    expect(replies(), isEmpty, reason: 'v2 is not public yet');

    // Publish to clax.dev… from the tile's menu publishes the current v2.
    await tester.longPress(
      find.byWidgetPredicate((w) => w is TileLabel && w.text == 'Plant log'),
    );
    await settle(tester, find.text('Publish to clax.dev…'));
    await tester.tap(find.text('Publish to clax.dev…'));
    await settleUntil(
      tester,
      () => replies().isNotEmpty,
      what: 'the publish landed and released the reply',
    );
    expect(replies(), ['$tid: Made it blue.']);
    // The hold writes its file once the reply is posted, off the UI.
    await settleUntil(tester, () => kept.written, what: 'the writes are done');
    expect(kept.asked, greaterThan(0));
    await tester.runAsync(() async {
      final left = (jsonDecode(
        await File('${dir.path}/held-replies.json').readAsString(),
      ) as Map)['replies'];
      expect(left, isEmpty);
      expect((await LineageStore(dir).entry(aid))!.link!.published, [1, 2]);
    });
  });

  testWidgets('Publish v2 and reply while clax.dev answers 503 holds up '
      'no other app, and its reply goes out once the publish lands', (
    tester,
  ) async {
    late String a, b, ta;
    await tester.runAsync(() async {
      await api.registerAppSession(installId: 'install', pid: 42);
      a = (await api.createArtifact(
        PublishRequest(title: 'Plant log', files: {'index.html': '<p>v1</p>'}),
      )).artifact.id;
      await api.publish(
        a,
        PublishRequest(ifVersion: 1, files: {'index.html': '<p>v2</p>'}),
      );
      for (final v in (await api.getArtifact(a)).versions) {
        remote.localVersion(a, v.n, contentSha256: v.contentSha256!);
      }
      b = (await api.createArtifact(
        PublishRequest(title: 'Runs', files: {'index.html': '<p>b1</p>'}),
      )).artifact.id;
      remote.connected('https://clax.dev');
      final store = LineageStore(dir);
      expect(
        await Publisher(
          remote: remote,
          exporter: Exporter(api: api, store: store, key: key),
          store: store,
          clock: FakeClock(),
        ).publish(a, version: 1),
        isA<Published>(),
      );
      ta = api
          .sendComment(a, 'make it blue', remote: 'clax.dev', queue: false)
          .threadId;
    });
    script.add(change(reply: 'Bigger now.', edits: [edit('b1', 'b2')]));
    final fake = clock = FakeClock();
    await tester.pumpWidget(AppRoot(services: services()));
    coreHost.set(const CoreReady());
    await settle(tester, _tile('Plant log'));
    await tester.tap(
      find.byWidgetPredicate((w) => w is TileLabel && w.text == 'Plant log'),
    );
    await settle(tester, find.byType(AppScreen));

    // clax.dev answers 503 to the push, twice.
    const down = ClaxApiError(
      status: 503,
      code: 'unavailable',
      message: 'clax.dev is down',
    );
    remote
      ..failNext('publish', down)
      ..failNext('publish', down);
    String? why;
    var answered = false;
    unawaited(
      hooks(tester).publishAndReply!(a, ta, 2).then((w) {
        why = w;
        answered = true;
      }),
    );
    await settleUntil(
      tester,
      () => answered,
      what: 'Publish and reply answers',
    );
    expect(why, contains('Couldn’t reach clax.dev'));
    expect(replies(), isEmpty);
    int? held;
    var read = false;
    unawaited(
      hooks(tester).heldReply!(a, ta).then((v) {
        held = v;
        read = true;
      }),
    );
    await settleUntil(tester, () => read, what: 'the held reply read');
    expect(held, 2, reason: 'the reply waits for v2');

    // A change on another app posts its reply at once.
    api.sendComment(b, 'make it b2');
    await settleUntil(
      tester,
      () => replies().any((r) => r.endsWith(': Bigger now.')),
      what: 'the other app’s reply posted',
    );
    expect(replies().where((r) => r.startsWith('$ta:')), isEmpty);

    // clax.dev recovers: the publish lands by itself, and the reply goes.
    for (var i = 0; i < 2; i++) {
      await tester.runAsync(() => fake.advance(const Duration(minutes: 5)));
      await tester.pump();
    }
    await settleUntil(
      tester,
      () => replies().contains('$ta: Made this change in version 2.'),
      what: 'the held reply went out after the publish landed',
    );
    await settleUntil(tester, () => kept.written, what: 'the writes are done');
    await tester.runAsync(() async {
      expect((await LineageStore(dir).entry(a))!.link!.published, [1, 2]);
    });
  });

  testWidgets('a held reply whose publish ends moved after the wait says '
      'why on its thread page', (tester) async {
    late String a, ta;
    await tester.runAsync(() async {
      await api.registerAppSession(installId: 'install', pid: 42);
      a = (await api.createArtifact(
        PublishRequest(title: 'Plant log', files: {'index.html': '<p>v1</p>'}),
      )).artifact.id;
      await api.publish(
        a,
        PublishRequest(ifVersion: 1, files: {'index.html': '<p>v2</p>'}),
      );
      for (final v in (await api.getArtifact(a)).versions) {
        remote.localVersion(a, v.n, contentSha256: v.contentSha256!);
      }
      remote.connected('https://clax.dev');
      final store = LineageStore(dir);
      await Publisher(
        remote: remote,
        exporter: Exporter(api: api, store: store, key: key),
        store: store,
        clock: FakeClock(),
      ).publish(a, version: 1);
      ta = api
          .sendComment(a, 'make it blue', remote: 'clax.dev', queue: false)
          .threadId;
    });
    final fake = clock = FakeClock();
    await tester.pumpWidget(AppRoot(services: services()));
    coreHost.set(const CoreReady());
    await settle(tester, _tile('Plant log'));
    await tester.tap(
      find.byWidgetPredicate((w) => w is TileLabel && w.text == 'Plant log'),
    );
    await settle(tester, find.byType(AppScreen));

    remote.failNext(
      'publish',
      const ClaxApiError(status: 503, code: 'unavailable', message: 'down'),
    );
    var answered = false;
    unawaited(
      hooks(tester).publishAndReply!(a, ta, 2).then((_) => answered = true),
    );
    await settleUntil(
      tester,
      () => answered,
      what: 'Publish and reply answers',
    );

    // The copy is changed on the web meanwhile: the publish ends moved.
    remote.publishThere(
      remote.copies.values.single.raid,
      contentSha256: 'sha256:${'9' * 64}',
    );
    await tester.runAsync(() => fake.advance(const Duration(minutes: 5)));
    String? note;
    await settleUntil(tester, () {
      unawaited(hooks(tester).heldNote!(a, 2).then((w) => note = w));
      return note != null;
    }, what: 'the held reply’s note');
    expect(note, contains('the public copy changed on clax.dev'));
    expect(replies(), isEmpty);
  });

  for (final comments in [false, true]) {
    testWidgets('a linked app opened reads its head once, and comments '
        '${comments ? 'on keep' : 'off take away'} its composer', (
      tester,
    ) async {
      late String aid;
      await tester.runAsync(() async {
        await api.registerAppSession(installId: 'install', pid: 42);
        aid = (await api.createArtifact(
          PublishRequest(
            title: 'Plant log',
            files: {'index.html': '<p>v1</p>'},
          ),
        )).artifact.id;
        remote.localVersion(
          aid,
          1,
          contentSha256: (await api.getArtifact(aid)).current!.contentSha256!,
        );
        remote.connected('https://clax.dev');
        final store = LineageStore(dir);
        await Publisher(
          remote: remote,
          exporter: Exporter(api: api, store: store, key: key),
          store: store,
          clock: FakeClock(),
        ).publish(aid, version: 1);
        // A first publish leaves comments off.
        remote.copies.values.single.comments = comments;
      });
      int heads() => remote.calls.where((c) => c == 'head').length;
      final setUpHeads = heads();
      await tester.pumpWidget(AppRoot(services: services()));
      coreHost.set(const CoreReady());
      // The launch's foreground check reads the head too.
      await settleUntil(
        tester,
        () => _tile('Plant log').evaluate().isNotEmpty && heads() > setUpHeads,
      );
      final before = heads();

      await tester.tap(
        find.byWidgetPredicate((w) => w is TileLabel && w.text == 'Plant log'),
      );
      await settleUntil(
        tester,
        () =>
            find.byType(AppScreen).evaluate().isNotEmpty &&
            hooks(tester).status.value[aid]?.head != null,
        what: 'the checker read the head',
      );
      expect(heads(), before + 1, reason: 'the checker reads it');
      // A second read, if one were coming, would come from the open: the
      // status it would set again.
      final read = hooks(tester).status.value[aid];
      await settleUntil(
        tester,
        () =>
            comments ||
            surfaces.last.hostCalls.whereType<ExitCommentMode>().isNotEmpty,
        what: 'the open took comments away',
      );
      expect(heads(), before + 1, reason: 'once');
      expect(hooks(tester).status.value[aid], same(read));
      final screen = surfaces.last;
      expect(
        screen.hostCalls.whereType<ExitCommentMode>(),
        comments ? isEmpty : isNotEmpty,
      );
    });
  }
}
