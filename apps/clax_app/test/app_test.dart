import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:ui' show AppExitResponse;

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/edit_result.dart';
import 'package:clax_app/agent/loop.dart';
import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/app.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/daemon/supervisor.dart';
import 'package:clax_app/platform/app_notice.dart';
import 'package:clax_app/platform/secure_key.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/inspector_surface.dart';
import 'package:clax_app/ui/launcher.dart';
import 'package:clax_app/ui/making_screen.dart' show sayFirstOutput;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'agent/loop_support.dart';
import 'support/fake_app_notice.dart';
import 'support/fake_supervisor.dart';
import 'support/fake_surface.dart';
import 'support/fake_voice.dart';

const _page = '<!doctype html><title>Runs</title><h1>Runs</h1>';

class _Key implements SecureKey {
  _Key([this.value]);
  String? value;

  @override
  Future<String?> read() async => value;

  @override
  Future<void> write(String? key) async => value = key;
}

/// Everything AppRoot is given, as fakes the test can reach.
class _Rig {
  _Rig({
    String? key,
    AppSettings? settings,
    this.cliSupported = false,
    this.webInspector = false,
    this.notice,
  }) : dir = Directory.systemTemp.createTempSync('app-test-'),
       key = _Key(key) {
    if (settings != null) {
      File('${dir.path}/settings.json').writeAsStringSync(
        '{"backend": "${settings.backend}", "model": "${settings.model}"}',
      );
    }
  }

  final Directory dir;
  final _Key key;
  final bool cliSupported;

  /// The open app's notice; none (as on macOS) when null.
  final FakeAppNotice? notice;

  /// Whether the app gets a [WebViewInspector] over fake surfaces
  /// ([inspectorSurfaces]).
  final bool webInspector;
  final inspectorSurfaces = <FakeAppSurface>[];
  final supervisor = FakeSupervisor();
  final api = FakeClaxApi();
  final timings = RecordingTimings();
  final agentTimings = RecordingTimings();
  final surfaces = <FakeAppSurface>[];
  final surfaceBases = <Uri>[];
  final backendFor = <String>[];

  /// What the agent answers; its calls can be held at a gate.
  late HeldBackend backend = HeldBackend([
    const EditResult(
      reply: 'Made a run tracker.',
      title: 'Runs',
      files: [FileWrite(path: 'index.html', content: _page)],
      edits: [],
      removed: [],
      db: [],
      capabilities: {'db': <String, Object?>{}},
      resolve: false,
    ),
  ], name: backendMessagesApi);

  AppServices services() => AppServices(
    supervisor: supervisor,
    api: (endpoint) => api,
    backend: (s, k) {
      backendFor.add(s.backend);
      return s.backend == backendClaudeCli && cliSupported
          ? HeldBackend([], name: backendClaudeCli)
          : backend;
    },
    voice: FakeVoice.unsupported('No speech here'),
    settings: SettingsStore(File('${dir.path}/settings.json')),
    key: key,
    timings: timings,
    agentTimings: agentTimings,
    surface: (base) {
      surfaceBases.add(base);
      final s = FakeAppSurface();
      surfaces.add(s);
      return s;
    },
    installId: 'install',
    supportDir: dir.path,
    prompts: PromptBuilder(rootBundle),
    cliSupported: cliSupported,
    notice: notice,
    inspector: webInspector
        ? WebViewInspector(
            surface: (_) {
              final s = FakeAppSurface();
              inspectorSurfaces.add(s);
              return s;
            },
          )
        : null,
    pid: 42,
  );

  void dispose() => dir.deleteSync(recursive: true);
}

void main() {
  late _Rig rig;

  Future<void> pumpApp(WidgetTester tester, _Rig r) async {
    rig = r;
    addTearDown(r.dispose);
    await tester.pumpWidget(AppRoot(services: r.services()));
    await tester.pump();
  }

  Future<void> up(WidgetTester tester, {int port = 7999, int pid = 100}) async {
    rig.supervisor.set(Up(endpoint(port: port, pid: pid)));
    await tester.pumpAndSettle();
  }

  /// Pumps through a route transition, without waiting for the page's
  /// cover (an endless progress indicator) to go.
  Future<void> pumpRoute(WidgetTester tester) async {
    for (var i = 0; i < 5; i++) {
      await tester.pump(const Duration(milliseconds: 200));
    }
  }

  /// Opens [aid] (a new tile) from the launcher, and has its shell report
  /// `ready`.
  Future<void> openApp(WidgetTester tester, String aid) async {
    rig.api.emit(ClaxEvent(type: 'version', data: {'artifact_id': aid}));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Runs'));
    await pumpRoute(tester);
    rig.surfaces.last.push(
      Ready(artifact: aid, version: 1, latest: 1, file: 'index.html'),
    );
    await tester.pumpAndSettle();
  }

  Future<void> submit(WidgetTester tester, String request) async {
    await tester.tap(find.text('New app'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), request);
    await tester.pump();
    await tester.tap(find.text('Make it'));
    await tester.pump();
  }

  testWidgets('a splash shows while the daemon starts; Up shows the launcher', (
    tester,
  ) async {
    await pumpApp(tester, _Rig(key: 'sk-ant-x'));
    expect(find.byKey(const ValueKey('splash')), findsOneWidget);
    expect(find.text('New app'), findsNothing);
    expect(rig.supervisor.ensureUpCalls, 1);

    await up(tester);
    expect(find.byKey(const ValueKey('splash')), findsNothing);
    expect(find.text('New app'), findsOneWidget);
  });

  testWidgets('a restart keeps the launcher; a failure shows the error screen '
      'with the tail, and Restart starts the daemon again', (tester) async {
    await pumpApp(tester, _Rig(key: 'sk-ant-x'));
    await up(tester);
    rig.supervisor.set(const Restarting(1, Duration(milliseconds: 200)));
    await tester.pumpAndSettle();
    expect(find.text('New app'), findsOneWidget);

    rig.supervisor.set(
      const Failed(
        'clax stopped 5 times within a minute; last: exited with '
        'code 1',
        ['error: database is locked', 'clax exiting'],
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('New app'), findsNothing);
    expect(find.text("Clax couldn't start"), findsOneWidget);
    expect(
      find.textContaining('clax stopped 5 times within a minute'),
      findsOneWidget,
    );
    expect(
      find.textContaining('error: database is locked\nclax exiting'),
      findsOneWidget,
    );

    await tester.tap(find.text('Restart'));
    await tester.pump();
    expect(rig.supervisor.ensureUpCalls, 2);
    rig.supervisor.set(const Starting());
    await tester.pump();
    expect(find.byKey(const ValueKey('splash')), findsOneWidget);
    await up(tester, pid: 101);
    expect(find.text('New app'), findsOneWidget);
  });

  testWidgets('a failure while an app is open goes back to the error screen', (
    tester,
  ) async {
    await pumpApp(tester, _Rig(key: 'sk-ant-x'));
    await up(tester);
    final aid = rig.api.seedArtifact('Runs', {'index.html': _page});
    await openApp(tester, aid);
    expect(find.byType(AppScreen), findsOneWidget);

    rig.supervisor.set(const Failed('clax did not come up', []));
    await tester.pumpAndSettle();
    expect(find.byType(AppScreen), findsNothing);
    expect(find.text("Clax couldn't start"), findsOneWidget);
    expect(find.text('clax wrote nothing.'), findsOneWidget);
  });

  testWidgets('resuming probes the daemon; detaching stops it', (tester) async {
    await pumpApp(tester, _Rig(key: 'sk-ant-x'));
    await up(tester);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await tester.pump();
    expect(rig.supervisor.onResumeCalls, 1);
    expect(rig.supervisor.stopCalls, 0);

    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.detached);
    await tester.pumpAndSettle();
    expect(rig.supervisor.stopCalls, 1);
  });

  testWidgets('quitting the app stops the worker and the daemon first', (
    tester,
  ) async {
    await pumpApp(tester, _Rig(key: 'sk-ant-x'));
    await up(tester);
    await until(() => rig.api.waitingPolls == 1);
    final response = await tester.binding.handleRequestAppExit();
    expect(response, AppExitResponse.exit);
    expect(rig.supervisor.stopCalls, 1);
    // The worker no longer polls: a released poll is not followed by
    // another.
    final polls = rig.api.callNames.where((n) => n == 'pollFeedback').length;
    rig.api.releasePolls();
    await tester.pump();
    expect(rig.api.callNames.where((n) => n == 'pollFeedback').length, polls);
    // A detach after the quit does not stop it twice.
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.detached);
    await tester.pump();
    expect(rig.supervisor.stopCalls, 1);
  });

  testWidgets('New runs the agent with its stages, then opens the app screen '
      'for the new ID; say-usable runs from Make to its first ready', (
    tester,
  ) async {
    await pumpApp(tester, _Rig(key: 'sk-ant-x'));
    await up(tester);
    rig.backend.gate = Completer<void>();
    await submit(tester, 'a tracker for my runs');
    await tester.pump(const Duration(milliseconds: 400));
    expect(rig.timings.started, [sayFirstOutput, sayUsable]);
    expect(find.text('Writing your app'), findsOneWidget);

    rig.backend.gate!.complete();
    await pumpRoute(tester);
    final created = (await rig.api.listArtifacts()).single;
    expect(find.byType(AppScreen), findsOneWidget);
    expect(rig.surfaces.single.loads, [
      AppScreen.appUrl(Uri.parse('http://127.0.0.1:7999'), created.id),
    ]);
    expect(rig.backend.seen.single.userText, contains('a tracker for my runs'));
    expect(rig.agentTimings.stopped.map((r) => r.name), [
      agentNewTiming,
    ], reason: 'the agent run goes to its own store');
    expect(rig.timings.stopped, isEmpty);

    rig.surfaces.single.push(
      Ready(artifact: created.id, version: 1, latest: 1, file: 'index.html'),
    );
    await tester.pumpAndSettle();
    expect(rig.timings.stopped.map((r) => r.name), [sayUsable]);
    expect(rig.timings.stopped.single.extra['artifact'], created.id);

    // Later readies (a reload, a new version) are not new apps.
    rig.surfaces.single.push(
      Ready(artifact: created.id, version: 1, latest: 1, file: 'index.html'),
    );
    await tester.pumpAndSettle();
    expect(rig.timings.stopped, hasLength(1));
  });

  testWidgets('Cancel stops the run and nothing is made', (tester) async {
    await pumpApp(tester, _Rig(key: 'sk-ant-x'));
    await up(tester);
    rig.backend.gate = Completer<void>();
    await submit(tester, 'a tracker for my runs');
    await rig.backend.arrived.future;
    await tester.pump(const Duration(milliseconds: 400));
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
    final cancelled = rig.backend.seen.single.cancelled;
    expect(cancelled, isNotNull);
    var fired = false;
    unawaited(cancelled!.then((_) => fired = true));
    await tester.pump();
    expect(fired, isTrue);
    expect(rig.api.callNames, isNot(contains('createArtifact')));
    expect(find.byType(AppScreen), findsNothing);
  });

  testWidgets('without an API key the launcher asks for one; Claude Code '
      'needs none', (tester) async {
    await pumpApp(tester, _Rig());
    await up(tester);
    expect(
      find.text('Add your Claude API key to start making apps'),
      findsOneWidget,
    );
    await tester.pumpWidget(const SizedBox());

    await pumpApp(
      tester,
      _Rig(
        settings: AppSettings(backend: backendClaudeCli),
        cliSupported: true,
      ),
    );
    await up(tester);
    expect(
      find.text('Add your Claude API key to start making apps'),
      findsNothing,
    );
  });

  testWidgets('a key added in Settings clears the banner', (tester) async {
    await pumpApp(tester, _Rig());
    await up(tester);
    await tester.tap(find.byTooltip('Settings'));
    await tester.pumpAndSettle();
    expect(find.text('Claude Code on this Mac'), findsNothing);
    rig.key.value = 'sk-ant-new';
    await tester.pageBack();
    await tester.pumpAndSettle();
    expect(
      find.text('Add your Claude API key to start making apps'),
      findsNothing,
    );
  });

  testWidgets('a restarted daemon reloads the open app; a new port opens it '
      'at the new address', (tester) async {
    await pumpApp(tester, _Rig(key: 'sk-ant-x'));
    await up(tester);
    final aid = rig.api.seedArtifact('Runs', {'index.html': _page});
    await openApp(tester, aid);
    final first = rig.surfaces.single;

    rig.supervisor.set(const Restarting(1, Duration(milliseconds: 200)));
    await up(tester, pid: 101);
    expect(first.reloads, 1);
    expect(rig.surfaces, hasLength(1));

    rig.supervisor.set(const Restarting(1, Duration(milliseconds: 200)));
    rig.supervisor.set(Up(endpoint(port: 8123, pid: 102)));
    await pumpRoute(tester);
    expect(rig.surfaces, hasLength(2));
    expect(first.disposed, isTrue);
    expect(rig.surfaces.last.loads, [
      AppScreen.appUrl(Uri.parse('http://127.0.0.1:8123'), aid),
    ]);
  });

  testWidgets(
    'on macOS the app owns the menu bar; App > Threads works only with an '
    'app open',
    (tester) async {
      final menus = <Object?>[];
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.menu,
        (call) async {
          if (call.method == 'Menu.setMenus') menus.add(call.arguments);
          return null;
        },
      );
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          SystemChannels.menu,
          null,
        ),
      );
      await pumpApp(tester, _Rig(key: 'sk-ant-x'));
      await up(tester);
      expect(_findItem(menus.last, 'Threads')!['enabled'], isFalse);

      final aid = rig.api.seedArtifact('Runs', {'index.html': _page});
      await openApp(tester, aid);
      final item = _findItem(menus.last, 'Threads')!;
      expect(item['enabled'], isTrue);
      await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
        'flutter/menu',
        SystemChannels.menu.codec.encodeMethodCall(
          MethodCall('Menu.selectedCallback', item['id']),
        ),
        (_) {},
      );
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('app-panel')), findsOneWidget);

      // Back closes the panel, then leaves the app.
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(find.byType(AppScreen), findsNothing);
      expect(_findItem(menus.last, 'Threads')!['enabled'], isFalse);
    },
    variant: TargetPlatformVariant.only(TargetPlatform.macOS),
  );

  testWidgets('once the daemon is up the app holds one stream on gallery, '
      'inbox and questions, which the launcher follows', (tester) async {
    await pumpApp(tester, _Rig(key: 'sk-ant-x'));
    expect(rig.api.callNames, isNot(contains('streamSubscribe')));
    await up(tester);
    final subscribed = [
      for (final c in rig.api.calls)
        if (c.name == 'streamSubscribe') c.args['topics'],
    ];
    expect(subscribed.first, ['gallery', 'inbox', 'questions']);
    final launcher = tester.widget<Launcher>(find.byType(Launcher));
    expect(launcher.stream, isNotNull);
    // The launcher's list follows the shared stream's gallery topic.
    final aid = rig.api.seedArtifact('Runs', {'index.html': _page});
    rig.api.emit(ClaxEvent(type: 'version', data: {'artifact_id': aid}));
    await tester.pumpAndSettle();
    expect(find.text('Runs'), findsOneWidget);
  });

  testWidgets('drafts a previous run left are deleted once the daemon is up', (
    tester,
  ) async {
    final r = _Rig(key: 'sk-ant-x');
    await r.api.createDraft(title: 'Draft', files: {'index.html': _page});
    await pumpApp(tester, r);
    await up(tester);
    for (var i = 0; i < 20 && (await r.api.listDrafts()).isNotEmpty; i++) {
      await tester.pump(const Duration(milliseconds: 50));
    }
    expect(await r.api.listDrafts(), isEmpty);
    expect(r.api.callNames, contains('deleteDraft'));
  });

  testWidgets('an inbox reply opens its app at the thread once the shell '
      'lists it; a version item at that version', (tester) async {
    await pumpApp(tester, _Rig(key: 'sk-ant-x'));
    await up(tester);
    final aid = rig.api.seedArtifact('Runs', {'index.html': _page});
    final launcher = tester.widget<Launcher>(find.byType(Launcher));

    launcher.onOpenTarget!(aid, thread: 't1');
    await pumpRoute(tester);
    final page = rig.surfaces.last;
    page.push(Ready(artifact: aid, version: 2, latest: 2, file: 'index.html'));
    await tester.pump();
    expect(page.js, isNot(contains(const OpenThread('t1').toJs())));
    ThreadSummary thread(String id) => ThreadSummary(
      id: id,
      status: 'open',
      summary: 'Bigger',
      sent: true,
      comments: 2,
      version: 1,
    );
    page.push(ThreadsMsg([thread('t0')]));
    await tester.pump();
    expect(page.js, isNot(contains(const OpenThread('t1').toJs())));
    page.push(ThreadsMsg([thread('t0'), thread('t1')]));
    page.push(ThreadsMsg([thread('t0'), thread('t1')]));
    await tester.pump();
    expect(
      page.js.where((j) => j == const OpenThread('t1').toJs()),
      hasLength(1),
    );

    await tester.binding.handlePopRoute();
    await pumpRoute(tester);
    launcher.onOpenTarget!(aid, version: 1);
    await pumpRoute(tester);
    final again = rig.surfaces.last;
    expect(again, isNot(same(page)));
    again.push(Ready(artifact: aid, version: 2, latest: 2, file: 'index.html'));
    await tester.pump();
    expect(again.js, contains(const LoadVersion(1).toJs()));
  });

  testWidgets('the first app screen of an install shows the hint, once; '
      "the bar's Voice is kept", (tester) async {
    final r = _Rig(key: 'sk-ant-x', notice: FakeAppNotice());
    await pumpApp(tester, r);
    await up(tester);
    final aid = rig.api.seedArtifact('Runs', {'index.html': _page});
    await openApp(tester, aid);
    var screen = tester.widget<AppScreen>(find.byType(AppScreen));
    expect(screen.firstRunHint, isTrue);
    expect(screen.notice, isNotNull);
    screen.onHintShown!();
    screen.onDictateChanged!(false);
    await tester.pump();
    await tester.binding.handlePopRoute();
    await pumpRoute(tester);
    await tester.tap(find.text('Runs'));
    await pumpRoute(tester);
    screen = tester.widget<AppScreen>(find.byType(AppScreen));
    expect(screen.firstRunHint, isFalse);
    final saved = jsonDecode(
      File('${r.dir.path}/settings.json').readAsStringSync(),
    );
    expect(saved, containsPair('app_hint_seen', true));
    expect(saved, containsPair('dictate_comments', false));
  });

  testWidgets('without a notice (macOS) there is no hint', (tester) async {
    await pumpApp(tester, _Rig(key: 'sk-ant-x'));
    await up(tester);
    final aid = rig.api.seedArtifact('Runs', {'index.html': _page});
    await openApp(tester, aid);
    final screen = tester.widget<AppScreen>(find.byType(AppScreen));
    expect(screen.firstRunHint, isFalse);
    expect(screen.notice, isNull);
  });

  testWidgets("the notice's actions reach the app on top; another app's open "
      'it first; the body brings its app back to the top', (tester) async {
    final notice = FakeAppNotice();
    await pumpApp(tester, _Rig(key: 'sk-ant-x', notice: notice));
    expect(notice.calls, ['clear'], reason: 'a stale notice goes at launch');
    await up(tester);
    final runs = rig.api.seedArtifact('Runs', {'index.html': _page});
    final notes = rig.api.seedArtifact('Notes', {'index.html': _page});
    await openApp(tester, runs);
    expect(notice.shown, '$runs Runs');

    notice.tap(runs, AppAction.threads);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('app-panel')), findsOneWidget);
    await tester.binding.handlePopRoute();
    await tester.pumpAndSettle();

    notice.tap(notes, AppAction.comment);
    await pumpRoute(tester);
    final top = tester
        .widgetList<AppScreen>(find.byType(AppScreen, skipOffstage: false))
        .last;
    expect(top.aid, notes);
    expect(top.initialAction, AppAction.comment);
    rig.surfaces.last.push(
      Ready(artifact: notes, version: 1, latest: 1, file: 'index.html'),
    );
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('comment-bar')), findsOneWidget);
    expect(notice.shown, '$notes Notes');

    // A page over the app: the notice's body brings the app back.
    unawaited(
      tester
          .state<NavigatorState>(find.byType(Navigator))
          .push(MaterialPageRoute<void>(builder: (_) => const Text('over'))),
    );
    await tester.pumpAndSettle();
    expect(notice.shown, isNull);
    notice.tap(notes, AppAction.open);
    await tester.pumpAndSettle();
    expect(find.text('over'), findsNothing);
    expect(notice.shown, '$notes Notes');
    expect(
      tester.widgetList(find.byType(AppScreen, skipOffstage: false)),
      hasLength(2),
      reason: 'not opened again',
    );
  });

  testWidgets('a notice tap that launched Clax acts once the daemon is up', (
    tester,
  ) async {
    final notice = FakeAppNotice();
    final r = _Rig(key: 'sk-ant-x', notice: notice);
    final aid = r.api.seedArtifact('Runs', {'index.html': _page});
    notice.launch = AppNoticeTap(aid, AppAction.threads);
    await pumpApp(tester, r);
    expect(find.byType(AppScreen), findsNothing);
    // The app opens as the daemon comes up; its cover spins meanwhile.
    rig.supervisor.set(Up(endpoint(port: 7999, pid: 100)));
    await pumpRoute(tester);
    final screen = tester.widget<AppScreen>(find.byType(AppScreen));
    expect(screen.aid, aid);
    expect(screen.initialAction, AppAction.threads);
  });

  testWidgets('a WebViewInspector is kept in the root, under the app, and '
      'warmed once the daemon is up', (tester) async {
    await pumpApp(tester, _Rig(key: 'sk-ant-x', webInspector: true));
    expect(rig.inspectorSurfaces, isEmpty);
    await up(tester);
    expect(find.byType(InspectorHost), findsOneWidget);
    // Warmed as the daemon came up, before any render.
    expect(rig.inspectorSurfaces.single.loads.single.path, '/a/000000000000');
    expect(find.text('New app'), findsOneWidget);
    expect(
      find.ancestor(
        of: find.byType(Launcher),
        matching: find.byType(InspectorHost),
      ),
      findsOneWidget,
    );
  });

  test('the backend follows the settings, and Claude Code only where it '
      'runs', () {
    final scratch = Directory.systemTemp.createTempSync('backend-');
    addTearDown(() => scratch.deleteSync(recursive: true));
    AgentBackend pick(String backend, {required bool cli}) => chooseBackend(
      AppSettings(backend: backend),
      _Key(),
      cliSupported: cli,
      scratch: scratch,
    );
    expect(pick(backendMessagesApi, cli: true).name, backendMessagesApi);
    expect(pick(backendClaudeCli, cli: true).name, backendClaudeCli);
    expect(pick(backendClaudeCli, cli: false).name, backendMessagesApi);
  });

  test('the tool loop runs on the Messages API, or a scripted runner, with '
      'an inspector; Claude Code and a missing inspector take the single '
      'call', () {
    final scratch = Directory.systemTemp.createTempSync('backend-');
    addTearDown(() => scratch.deleteSync(recursive: true));
    final inspector = WebViewInspector(surface: (_) => FakeAppSurface());
    AgentBackend pick(String backend) => chooseBackend(
      AppSettings(backend: backend),
      _Key('sk-ant-x'),
      cliSupported: true,
      scratch: scratch,
    );
    final api = pick(backendMessagesApi);
    final cli = pick(backendClaudeCli);
    expect(usesToolLoop(api, inspector: inspector), isTrue);
    expect(usesToolLoop(cli, inspector: inspector), isFalse);
    expect(usesToolLoop(api, inspector: null), isFalse);
    expect(
      usesToolLoop(
        HeldBackend([], name: backendMessagesApi),
        inspector: inspector,
        runner: () => throw StateError('unused'),
      ),
      isTrue,
    );
  });

  test('a scripted agent file reads results and errors', () {
    final script = readAgentScript('''
      [{"reply": "Made it.", "title": "Runs", "files": [{"path": "index.html",
        "content": "<h1>Runs</h1>"}], "edits": [], "removed": [], "db": [],
        "capabilities_json": "{\\"db\\": {}}", "resolve": false},
       {"error": "The model is busy"}]''');
    expect(script, hasLength(2));
    expect((script[0] as EditResult).title, 'Runs');
    expect((script[0] as EditResult).capabilities, {'db': <String, Object?>{}});
    expect((script[1] as AgentError).message, 'The model is busy');
    expect(() => readAgentScript('{}'), throwsFormatException);
  });
}

Map<Object?, Object?>? _findItem(Object? tree, String label) {
  if (tree is Map) {
    if (tree['label'] == label) return tree;
    for (final v in tree.values) {
      final f = _findItem(v, label);
      if (f != null) return f;
    }
  } else if (tree is List) {
    for (final v in tree) {
      final f = _findItem(v, label);
      if (f != null) return f;
    }
  }
  return null;
}
