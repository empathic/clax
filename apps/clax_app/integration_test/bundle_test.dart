// App bundles end to end on macOS (bundles spec §13): the real embedded
// daemon, locked as the app runs it (`--require-credential`), the app's
// real WebViewInspector, and a scripted agent making the app.
//
// An author makes a notes app, exports it, and a recipient on the same
// daemon (its own lineage store, key and app session) installs it; the
// author ships v2 (a `migrate.js` that reshapes the notes and writes the
// person's private `data/users/` document, and a test), v3 (a failing
// test) and v4 (a `migrate.js` that throws part way on the live app). The
// recipient updates to v2, keeps v2 through v3, and is rolled back from v4;
// an older bundle is refused, and a modified copy is offered the choice
// between replacing its changes and keeping them.
//
// Run from apps/clax_app, alone (one test file per `flutter test -d macos`
// run): `flutter test integration_test/bundle_test.dart -d macos`, or
// `just app-integration`; the scratch directory is rig.dart's.
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:clax_app/agent/runner/scripted_runner.dart';
import 'package:clax_app/agent/scripted_backend.dart';
import 'package:clax_app/agent/tools/toolbox.dart';
import 'package:clax_app/app.dart';
import 'package:clax_app/bundle/bundle_reader.dart';
import 'package:clax_app/bundle/doc_snapshot.dart';
import 'package:clax_app/bundle/exporter.dart';
import 'package:clax_app/bundle/installer.dart';
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/bundle/trust.dart';
import 'package:clax_app/bundle/updater.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/http_api.dart';
import 'package:clax_app/daemon/supervisor.dart';
import 'package:clax_app/platform/bundle_key.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/daemon_cookie.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/inspector_surface.dart';
import 'package:clax_app/ui/webview_surface.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'rig.dart';
import 'tool_script.dart' show RecordingRunner, scriptIcon;

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('export, install, update, refuse and roll back app bundles', (
    tester,
  ) async {
    final runner = RecordingRunner([_makeRun]);
    late final Rig rig;
    final cookie = DaemonKeyCookie(token: () => rig.token());
    final inspector = WebViewInspector(
      surface: (base) => WebViewAppSurface(base: base, keyCookie: cookie),
    );
    rig = await Rig.create(
      backend: ScriptedBackend(const []),
      runner: () => runner,
      inspector: inspector,
      keyCookie: cookie,
    );
    addTearDown(rig.close);

    await runClaxApp(rig.services);
    await rig.until(tester, 'the launcher', () => find.text('New app'));

    // --- The author makes the app on the tool loop; its page seeds two
    // notes of the first shape, `{text}`.
    await tester.tap(find.text('New app'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'Keep my notes');
    await tester.pump();
    await tester.tap(find.text('Make it'));
    final first = await rig.until<Ready>(
      tester,
      'the new app\'s first ready',
      () => rig.messages.whereType<Ready>().firstOrNull,
    );
    final aid = first.artifact;
    expect(
      runner.outputs(1, 'finish').single,
      isA<ToolOk>(),
      reason: '${runner.calls}',
    );
    await rig.until(
      tester,
      'the page seeding its notes',
      () async => (await rig.api.getDoc(aid, 'notes/n2')) != null,
    );
    // Back to the launcher: the author's page stops writing.
    Navigator.of(tester.element(find.byType(AppScreen))).pop();
    await rig.until(
      tester,
      'the launcher again',
      () => find.byType(AppScreen).evaluate().isEmpty,
    );
    expect(await _notes(rig.api, aid), _v1Notes);

    final endpoint = switch (rig.supervisor.state) {
      Up(:final endpoint) => endpoint,
      final s => throw StateError('the daemon is not up: $s'),
    };
    // The author publishes as the app does, in its app session.
    await rig.api.registerAppSession(
      installId: 'f10-integration',
      pid: pid,
      cwd: rig.dir.path,
    );
    final authorStore = LineageStore(
      Directory('${rig.dir.path}/author')..createSync(),
    );
    final exporter = Exporter(
      api: rig.api,
      store: authorStore,
      key: BundleKey(_MemorySeed()),
    );

    // The recipient: its own client and app session, store and key.
    final recipientDir = Directory('${rig.dir.path}/recipient')..createSync();
    final recipientApi = HttpClaxApi.loopback(() => endpoint);
    addTearDown(recipientApi.close);
    Future<void> session() => recipientApi.registerAppSession(
      installId: 'bundle-recipient',
      pid: pid,
      cwd: recipientDir.path,
    );
    await session();
    final recipientStore = LineageStore(recipientDir);
    final installer = Installer(
      api: recipientApi,
      store: recipientStore,
      key: BundleKey(_MemorySeed()),
    );
    final updater = Updater(
      api: recipientApi,
      store: recipientStore,
      inspector: inspector,
      baseUrl: () => endpoint.base,
      snapshots: DocSnapshots(recipientApi, recipientDir),
      ensureSession: session,
      log: (l) => debugPrint('bundle test updater: $l'),
    );

    // --- Export v1, without and with the seed data.
    final bare = await _wait(tester, 'export', exporter.export(aid));
    expect(bare.manifest.version, 1);
    expect(bare.manifest.seed, isNull);
    expect((await BundleReader.read(bare.bytes)).seed, isNull);
    final v1 = await _wait(
      tester,
      'export with data',
      exporter.export(aid, includeData: true, authorName: 'Author'),
    );
    expect(v1.manifest.version, 1, reason: 'the data switch keeps v1');
    expect(v1.manifest.lineage, bare.manifest.lineage);
    expect(v1.fileName, 'notes.clax');
    final authorV1 = (await rig.api.getArtifact(aid)).current!;
    expect(v1.manifest.contentSha256, authorV1.contentSha256);
    expect({
      for (final d in (await BundleReader.read(v1.bytes)).seed!) d.path: d.data,
    }, _v1Notes);

    // --- Install it as a second app; the daemon's content hash matches.
    final opened1 = await _wait(tester, 'open v1', installer.open(v1.bytes));
    expect(opened1.decision.answer, TrustAnswer.install);
    expect(opened1.fromSelf, isFalse);
    final rid = await _wait(tester, 'install', installer.install(opened1).then((i) => i.aid));
    expect(rid, isNot(aid));
    final installed = await recipientApi.getArtifact(rid);
    expect(installed.current!.contentSha256, v1.manifest.contentSha256);
    expect(installed.current!.contentSha256, authorV1.contentSha256);
    expect(installed.artifact.title, 'Notes');
    expect(await _notes(recipientApi, rid), _v1Notes);
    expect(await fromBundleOf(recipientStore)(rid), isTrue);
    expect(await fromBundleOf(authorStore)(aid), isFalse);
    final entry1 = (await recipientStore.entry(rid))!;
    expect(entry1.role, LineageRole.installed);
    expect(entry1.version, 1);
    expect(await recipientStore.keptBundle(rid), v1.bytes);

    // --- v2: a migrate.js reshaping `{text}` to `{title}` and writing the
    // person's `data/users/<id>/prefs`, and a test reading the new shape.
    await _publish(
      rig.api,
      aid,
      PublishRequest(
        icon: 'book',
        files: {
          'index.html': _notesHtml(seeds: false),
          'migrate.js': _migrateV2,
          'probe.js': _probe,
          'tests/notes.test.js': _testV2,
        },
      ),
    );
    final v2 = await _wait(tester, 'export v2', exporter.export(aid));
    expect(v2.manifest.version, 2);
    expect(v2.manifest.lineage, v1.manifest.lineage);

    final opened2 = await _wait(tester, 'open v2', installer.open(v2.bytes));
    expect(opened2.decision.answer, TrustAnswer.update);
    expect(opened2.decision.aid, rid);
    expect(opened2.decision.modified, isFalse);
    final steps = <UpdateStep>[];
    final up2 = await _wait(
      tester,
      'update to v2',
      updater.update(rid, opened2.bundle, onStep: steps.add),
    );
    expect(_describe(up2), 'landed v2');
    expect(steps, [
      UpdateStep.data,
      UpdateStep.migrate,
      UpdateStep.page,
      UpdateStep.tests,
      UpdateStep.publish,
      UpdateStep.liveMigrate,
    ]);
    final afterV2 = await recipientApi.getArtifact(rid);
    expect(afterV2.current!.contentSha256, v2.manifest.contentSha256);
    expect(afterV2.artifact.icon, 'book');
    expect(await _notes(recipientApi, rid), _v2Notes);
    expect(await _private(tester, inspector, endpoint.base, rid), _v2Private);
    final entry2 = (await recipientStore.entry(rid))!;
    expect(entry2.version, 2);
    expect(entry2.released, [
      v1.manifest.contentSha256,
      v2.manifest.contentSha256,
    ]);
    await _noDrafts(tester, recipientApi);

    // --- v3: its test fails on the recipient's data; v2 stays.
    await _publish(
      rig.api,
      aid,
      PublishRequest(
        files: {
          'index.html': _notesHtml(seeds: false),
          'tests/notes.test.js': _testFails,
        },
      ),
    );
    final v3 = await _wait(tester, 'export v3', exporter.export(aid));
    expect(v3.manifest.version, 3);
    final opened3 = await _wait(tester, 'open v3', installer.open(v3.bytes));
    expect(opened3.decision.answer, TrustAnswer.update);
    final up3 = await _wait(
      tester,
      'update to v3',
      updater.update(rid, opened3.bundle),
    );
    expect(_describe(up3), startsWith('check failed at tests: a check failed'));
    final afterV3 = await recipientApi.getArtifact(rid);
    expect(afterV3.artifact.currentVersion, afterV2.artifact.currentVersion);
    expect(afterV3.current!.contentSha256, v2.manifest.contentSha256);
    expect(await _notes(recipientApi, rid), _v2Notes);
    expect((await recipientStore.entry(rid))!.version, 2);
    await _noDrafts(tester, recipientApi);

    // --- v4: a new title, icon and capability, and a migrate.js that
    // passes on the check's copy (which has no private documents) but
    // throws part way on the live app, after reshaping the notes, adding
    // one and writing two private documents.
    await _publish(
      rig.api,
      aid,
      PublishRequest(
        title: 'Notes Four',
        icon: 'star',
        capabilities: const {
          'db': <String, Object?>{},
          'notify': <String, Object?>{},
          'user': <String, Object?>{},
        },
        files: {
          'index.html': _notesHtml(seeds: false, heading: 'Notes Four'),
          'migrate.js': _migrateV4,
          'tests/notes.test.js': _testV4,
        },
      ),
    );
    final v4 = await _wait(tester, 'export v4', exporter.export(aid));
    expect(v4.manifest.version, 4);
    final opened4 = await _wait(tester, 'open v4', installer.open(v4.bytes));
    expect(opened4.decision.answer, TrustAnswer.update);
    final up4 = await _wait(
      tester,
      'update to v4',
      updater.update(rid, opened4.bundle),
    );
    expect(_describe(up4), startsWith('rolled back to v2, restored'));
    expect((up4 as UpdateRolledBack).details.join(), contains('part way'));
    final afterV4 = await recipientApi.getArtifact(rid);
    expect(
      afterV4.artifact.currentVersion,
      afterV2.artifact.currentVersion + 2,
      reason: 'the update, then base published again',
    );
    expect(afterV4.current!.contentSha256, v2.manifest.contentSha256);
    expect(afterV4.artifact.title, 'Notes');
    expect(afterV4.artifact.icon, 'book');
    expect(afterV4.artifact.capabilities, {
      'db': <String, Object?>{},
      'user': <String, Object?>{},
    });
    expect(await _notes(recipientApi, rid), _v2Notes);
    expect(await _private(tester, inspector, endpoint.base, rid), _v2Private);
    expect(DocSnapshots(recipientApi, recipientDir).has(rid), isFalse);
    expect((await recipientStore.entry(rid))!.version, 2);
    await _noDrafts(tester, recipientApi);

    // --- Older and equal bundles are not installed.
    final again1 = await _wait(tester, 'reopen v1', installer.open(v1.bytes));
    expect(again1.decision.answer, TrustAnswer.older);
    expect(again1.decision.aid, rid);
    final again2 = await _wait(tester, 'reopen v2', installer.open(v2.bytes));
    expect(again2.decision.answer, TrustAnswer.current);

    // --- The rolled-back app is unmodified; once the recipient changes
    // it, v4 offers the choice, and Keep mine stops updates.
    expect(
      (await _wait(
        tester,
        'reopen v4',
        installer.open(v4.bytes),
      )).decision.modified,
      isFalse,
    );
    await _publish(
      recipientApi,
      rid,
      PublishRequest(
        files: {'index.html': _notesHtml(seeds: false, heading: 'My notes')},
      ),
    );
    final mod = await _wait(
      tester,
      'open v4 modified',
      installer.open(v4.bytes),
    );
    expect(mod.decision.answer, TrustAnswer.update);
    expect(mod.decision.modified, isTrue);
    expect(mod.decision.stopped, isFalse);
    await updater.keepMine(rid);
    final kept = await _wait(tester, 'open v4 kept', installer.open(v4.bytes));
    expect(kept.decision.answer, TrustAnswer.update);
    expect(kept.decision.stopped, isTrue);
    expect((await recipientStore.entry(rid))!.updatesStopped, isTrue);
    expect(await recipientApi.listArtifacts(), hasLength(2));

    // Unmount the app: its worker stops before the daemon does.
    await tester.pumpWidget(const SizedBox());
  }, timeout: const Timeout(Duration(minutes: 10)));
}

/// The first shape's notes, which the v1 page seeds.
const _v1Notes = {
  'notes/n1': {'text': 'hello'},
  'notes/n2': {'text': 'world'},
};

/// The notes after v2's migration.
const _v2Notes = {
  'notes/n1': {'title': 'hello', 'shape': 2},
  'notes/n2': {'title': 'world', 'shape': 2},
};

/// The person's private documents after v2's migration from v1.
const _v2Private = {
  'prefs': {'seen': 2, 'from': 1},
  'extra': null,
};

/// The notes page: a heading and the list of notes (`title`, else
/// `text`); the first version seeds two notes on its first visit.
String _notesHtml({required bool seeds, String heading = 'Notes'}) =>
    '''<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>$heading</title>
<style>
:root { --bg: #ffffff; --fg: #1d1d1f; }
@media (prefers-color-scheme: dark) { :root { --bg: #1d1d1f; --fg: #f5f5f7; } }
body { margin: 0; padding: 0 16px; background: var(--bg); color: var(--fg); font: 16px/1.4 system-ui, sans-serif; }
</style>
</head>
<body>
<h1>$heading</h1>
<ul id="list"></ul>
<script>
(async () => {
  const db = await window.claude.use("db");
  if (!db) return;
  const notes = db.collection("notes");
  let snap = await notes.get();
  ${seeds ? '''if (snap.empty) {
    await db.doc("notes/n1").set({ text: "hello" });
    await db.doc("notes/n2").set({ text: "world" });
    snap = await notes.get();
  }''' : ''}
  const list = document.getElementById("list");
  for (const d of snap.docs) {
    const v = d.data() || {};
    const li = document.createElement("li");
    li.textContent = String(v.title ?? v.label ?? v.text);
    list.appendChild(li);
  }
})();
</script>
</body>
</html>
''';

const _testV1 = '''test("shows the heading", async () => {
  await ui.waitFor(() => ui.text("h1") === "Notes", 3000);
  expect(ui.text("h1")).toBe("Notes");
});
''';

const _testV2 = '''test("shows the migrated titles", async () => {
  await ui.waitFor(() => ui.text("#list").includes("hello"), 3000);
  expect(ui.text("#list")).toContain("world");
});
''';

const _testFails = '''test("fails on purpose", async () => {
  expect(ui.text("h1")).toBe("Not the heading");
});
''';

const _testV4 = '''test("shows the heading", async () => {
  await ui.waitFor(() => ui.text("h1") === "Notes Four", 3000);
  expect(ui.text("h1")).toBe("Notes Four");
});
''';

/// v2's migration: `{text}` notes become `{title, shape: 2}`; the person's
/// `data/users/<id>/prefs` records the run.
const _migrateV2 = '''export default async function migrate(db, { from, to }) {
  const me = "data/users/" + (await (await window.claude.use("user")).id());
  const snap = await db.collection("notes").get();
  for (const d of snap.docs) {
    const v = d.data();
    if (typeof v.text === "string") {
      await db.doc("notes/" + d.id).set({ title: v.text, shape: 2 });
    }
  }
  await db.doc(me + "/prefs").set({ seen: to, from });
}
''';

/// v4's migration: reshapes every note, adds one, writes two private
/// documents, then throws when the person's prefs existed before it ran
/// (the live app, never the check's copy).
const _migrateV4 = '''export default async function migrate(db, { from, to }) {
  const me = "data/users/" + (await (await window.claude.use("user")).id());
  const prefs = await db.doc(me + "/prefs").get();
  const snap = await db.collection("notes").get();
  for (const d of snap.docs) {
    const v = d.data();
    await db.doc("notes/" + d.id).set({ label: v.title ?? v.text, shape: 4 });
  }
  await db.doc("notes/n3").set({ label: "added", shape: 4 });
  await db.doc(me + "/prefs").set({ seen: to, from });
  await db.doc(me + "/extra").set({ x: 1 });
  if (prefs.exists) throw new Error("stopped part way");
}
''';

/// Reports the person's private documents, which the token cannot read,
/// as its error: run with `Inspector.runMigration(file: 'probe.js')`.
const _probe = '''export default async function probe(db) {
  const me = "data/users/" + (await (await window.claude.use("user")).id());
  const read = async (id) => {
    const s = await db.doc(me + "/" + id).get();
    return s.exists ? s.data() : null;
  };
  throw new Error("PROBE " + JSON.stringify({
    prefs: await read("prefs"),
    extra: await read("extra"),
  }));
}
''';

/// The make: the page and its test, named with `db` declared, rendered,
/// with an icon.
final _makeRun = [
  ScriptedCall('write_file', {
    'path': 'index.html',
    'content': _notesHtml(seeds: true),
  }),
  const ScriptedCall('write_file', {
    'path': 'tests/notes.test.js',
    'content': _testV1,
  }),
  const ScriptedCall('set_meta', {
    'title': 'Notes',
    'capabilities_json': '{"db": {}, "user": {}}',
  }),
  const ScriptedCall('render'),
  const ScriptedCall('write_file', {'path': 'icon.svg', 'content': scriptIcon}),
  const ScriptedCall('finish', {'reply': 'Made a notes app.', 'resolve': true}),
];

/// Publishes [r] over [aid]'s live version.
Future<PublishResult> _publish(
  ClaxApi api,
  String aid,
  PublishRequest r,
) async {
  r.ifVersion = (await api.getArtifact(aid)).artifact.currentVersion;
  return api.publish(aid, r);
}

/// [aid]'s `notes/` documents at the owner's level, by path.
Future<Map<String, Object?>> _notes(ClaxApi api, String aid) async => {
  await for (final d in ownerDocs(api, aid))
    if (d.path.startsWith('notes/')) d.path: d.data,
};

/// The person's private `prefs` and `extra` documents of [aid], read in
/// the migration view by `probe.js`.
Future<Object?> _private(
  WidgetTester tester,
  WebViewInspector inspector,
  Uri base,
  String aid,
) async {
  final run = await _wait(
    tester,
    'the private documents probe',
    inspector.runMigration(
      base.replace(pathSegments: ['a', aid], query: 'app=1&inspect=1'),
      file: 'probe.js',
      from: 1,
      to: 1,
    ),
  );
  final m = RegExp(r'PROBE (\{.*\})').firstMatch(run.error ?? '');
  if (m == null) fail('the probe did not report: ${run.error}');
  return jsonDecode(m.group(1)!);
}

Future<void> _noDrafts(WidgetTester tester, ClaxApi api) async {
  expect(await _wait(tester, 'the drafts', api.listDrafts()), isEmpty);
}

/// An update's outcome in one line, for the expectations' messages.
String _describe(UpdateOutcome o) => switch (o) {
  UpdateLanded(:final version) => 'landed v$version',
  UpdateCheckFailed(:final step, :final reason, :final details) =>
    'check failed at ${step.name}: $reason $details',
  UpdateRefused(:final message, :final details) => 'refused: $message $details',
  UpdateRolledBack(:final backOn, :final restored, :final details) =>
    'rolled back to v$backOn, ${restored ? 'restored' : 'not restored'} '
        '$details',
  UpdateCancelled() => 'cancelled',
};

/// Waits for [f], pumping frames meanwhile (the inspector's WebView is in
/// the app's tree), at most three minutes.
Future<T> _wait<T>(WidgetTester tester, String what, Future<T> f) async {
  var done = false;
  late T value;
  Object? error;
  StackTrace? trace;
  unawaited(
    f.then(
      (v) {
        value = v;
        done = true;
      },
      onError: (Object e, StackTrace s) {
        error = e;
        trace = s;
        done = true;
      },
    ),
  );
  final end = DateTime.now().add(const Duration(minutes: 3));
  while (!done) {
    await tester.pump();
    if (DateTime.now().isAfter(end)) fail('timed out waiting for $what');
    await Future<void>.delayed(const Duration(milliseconds: 50));
  }
  if (error case final e?) Error.throwWithStackTrace(e, trace!);
  return value;
}

/// A bundle signing seed kept in memory: each store's own install key.
class _MemorySeed implements BundleSeedStorage {
  String? _seed;

  @override
  Future<String?> read() async => _seed;

  @override
  Future<void> write(String seed) async => _seed = seed;
}
