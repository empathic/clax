import 'dart:convert';
import 'dart:io';

import 'package:clax_app/agent/inspector.dart';
import 'package:clax_app/agent/loop.dart';
import 'package:clax_app/agent/plan.dart';
import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/agent/runner/scripted_runner.dart';
import 'package:clax_app/agent/unfinished.dart';
import 'package:clax_app/bundle/bundle_reader.dart';
import 'package:clax_app/bundle/updater.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/remote/fake_remote_api.dart';
import 'package:clax_app/remote/merge.dart';
import 'package:clax_app/remote/models.dart';
import 'package:clax_app/remote/publisher.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../agent/loop_support.dart';
import '../agent/tools/support.dart' show StepClock;
import '../bundle/services_support.dart';
import '../support/fake_clock.dart';

/// Combining versions (spec 2026-10-21-app-push-pull §5.7 "Modified
/// copy", §5.10): the agent's merge on a draft, Bring vN into mine, and a
/// diverged app's Take theirs, Combine and Publish as a new app.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late Directory tmp;
  late Install me;
  late FakeRemoteApi remote;
  late Remote r;

  setUp(() async {
    tmp = Directory.systemTemp.createTempSync('remote-merge');
    me = Install(tmp);
    await me.ready();
    remote = FakeRemoteApi();
    r = remote.connected('https://clax.example');
  });
  tearDown(() => tmp.deleteSync(recursive: true));

  Publisher publisher() => Publisher(
    remote: remote,
    exporter: me.exporter,
    store: me.store,
    clock: FakeClock(),
  );

  Merger merger({MergeRun? run}) => Merger(
    api: me.api,
    store: me.store,
    updater: me.updater,
    publisher: publisher(),
    run: run,
  );

  /// A merge that answers [result], telling [saw] what it was given.
  MergeRun answering(
    MergeResult result, {
    void Function(
      Map<String, Uint8List> mine,
      Map<String, Uint8List> theirs,
      String from,
      Map<String, Object?>? theirsCapabilities,
    )?
    saw,
  }) =>
      (
        aid, {
        required mine,
        required theirs,
        required base,
        required from,
        required theirsCapabilities,
        required source,
        cancelled,
      }) async {
        saw?.call(mine, theirs, from, theirsCapabilities);
        return result;
      };

  Future<String> live(String aid) async {
    final n = me.api.currentVersion(aid);
    return utf8.decode((await me.api.readFileBytes(aid, n, 'index.html'))!);
  }

  Future<VersionInfo> version(String aid, int n) async =>
      (await me.api.getArtifact(aid)).versions.firstWhere((v) => v.n == n);

  group('Bring vN into mine', () {
    late String aid;
    late Bundle v2;

    setUp(() async {
      final v1 = await makeBundle(lineage: lineageA);
      aid = (await me.installer.install(await me.installer.open(v1.bytes))).aid;
      // The person's own change on top of v1.
      await me.api.publish(
        aid,
        PublishRequest(
          ifVersion: me.api.currentVersion(aid),
          files: {'index.html': '<p>v1</p><p>mine</p>'},
        ),
      );
      v2 = await BundleReader.read(
        (await makeBundle(
          lineage: lineageA,
          version: 2,
          files: {'index.html': '<h1>Plants</h1><p>v1</p>'},
        )).bytes,
      );
      me.api.calls.clear();
    });

    test('merge_runs_on_draft_and_checks', () async {
      const combine = [
        ScriptedCall('str_replace', {
          'path': 'index.html',
          'old_str': '<p>v1</p><p>mine</p>',
          'new_str': '<h1>Plants</h1><p>v1</p><p>mine</p>',
        }),
        ScriptedCall('render', {'file': null}),
        ScriptedCall('finish', {
          'note': 'Kept your line and took the heading.',
        }),
      ];
      // One run for the merge alone, one for Bring into mine.
      final runner = HeldRunner([combine, combine]);
      final sid = (await me.api.registerAppSession(
        installId: 'merge',
        pid: 2,
      )).id;
      final loop = AgentLoop(
        api: me.api,
        runner: () => runner,
        prompts: PromptBuilder(rootBundle),
        model: () => 'claude-haiku-5-5',
        timings: RecordingTimings(),
        inspector: me.inspector,
        baseUrl: () => Uri.parse('http://127.0.0.1:7999'),
        plans: PlanStore(),
        unfinished: MemoryUnfinishedChanges(),
        clock: StepClock(),
      );
      final before = me.api.currentVersion(aid);

      // The merge alone makes no version: it answers the combined files.
      final merged = await loop.merge(
        aid,
        mine: {'index.html': utf('<p>v1</p><p>mine</p>')},
        theirs: v2.files,
        base: {'index.html': utf('<p>v1</p>')},
        from: 'v2 by Sam',
        theirsCapabilities: v2.manifest.capabilities,
        session: sid,
      );
      expect(merged, isA<Merged>());
      merged as Merged;
      expect(
        utf8.decode(merged.files['index.html']!),
        '<h1>Plants</h1><p>v1</p><p>mine</p>',
      );
      expect(merged.note, 'Kept your line and took the heading.');
      expect(me.api.currentVersion(aid), before);
      // The agent saw its version, and what v2 changed from the base.
      expect(runner.text(0), contains('<p>v1</p><p>mine</p>'));
      expect(runner.text(0), contains('+<h1>Plants</h1><p>v1</p>'));
      expect(runner.text(0), contains('v2 by Sam'));

      // Bring into mine: the merge, then the update's checks on a draft
      // holding a copy of the data, then the offer, then the publish.
      final checks = me.inspector.calls.length;
      int? atOffer;
      String? offered;
      final out = await merger(run: loop.merge).bringIntoMine(
        aid,
        v2,
        confirm: (m) async {
          atOffer = me.api.currentVersion(aid);
          offered = m.note;
          return true;
        },
      );
      expect(out, isA<UpdateLanded>(), reason: _say(out));
      // Nothing was published before the person took the offer, which
      // showed the merge's note.
      expect(atOffer, before);
      expect(offered, 'Kept your line and took the heading.');
      final n = me.api.currentVersion(aid);
      expect(n, before + 1);
      // It landed release 2.
      expect((out as UpdateLanded).version, 2);
      expect(await live(aid), '<h1>Plants</h1><p>v1</p><p>mine</p>');
      final made = await version(aid, n);
      expect(made.label, 'Your version with v2');
      expect(made.note, contains('Kept your line and took the heading.'));
      // The update's check ran on a draft (page loaded), deleted after.
      expect(
        me.api.calls.where((c) => c.name == 'createDraft'),
        hasLength(greaterThanOrEqualTo(2)),
      );
      expect(me.inspector.calls.length, greaterThan(checks));
      expect(await me.api.listDrafts(), isEmpty);
      // Release 2 is taken; the copy stays modified on it.
      final e = (await me.store.entry(aid))!;
      expect(e.version, 2);
      expect(
        e.isModified(
          contentSha256: made.contentSha256!,
          capabilities: (await me.api.getArtifact(aid)).artifact.capabilities,
        ),
        isTrue,
      );
    });

    test('Discard on the offer leaves the app as it was', () async {
      final before = me.api.currentVersion(aid);
      final out = await merger(
        run:
            (
              aid, {
              required mine,
              required theirs,
              required base,
              required from,
              required theirsCapabilities,
              required source,
              cancelled,
            }) async => Merged(
              files: {'index.html': utf('<h1>Plants</h1><p>mine</p>')},
              title: 'Plant log',
              capabilities: const {'db': {}},
              note: 'Both.',
            ),
      ).bringIntoMine(aid, v2, confirm: (_) async => false);
      expect(out, isA<UpdateCancelled>());
      expect(me.api.currentVersion(aid), before);
      expect(await live(aid), '<p>v1</p><p>mine</p>');
      expect((await me.store.entry(aid))!.version, 1);
    });

    test('a merge that gives up changes nothing', () async {
      VersionSource? sawSource;
      final out = await merger(
        run:
            (
              aid, {
              required mine,
              required theirs,
              required base,
              required from,
              required theirsCapabilities,
              required source,
              cancelled,
            }) async {
              sawSource = source;
              return const MergeFailed('they rewrite the same table');
            },
      ).bringIntoMine(aid, v2);
      // The version brought in is its signer's, for the person's trust.
      expect(sawSource!.key, v2.manifest.author.key);
      expect(sawSource!.who, v2.manifest.author.name);
      expect(sawSource!.account, isNull);
      expect(out, isA<UpdateRefused>());
      expect(
        (out as UpdateRefused).message,
        'Claude couldn’t bring v2 into yours: they rewrite the same table',
      );
      expect(await live(aid), '<p>v1</p><p>mine</p>');
    });

    test('a merge declaring what neither version declares is refused', () async {
      final before = me.api.currentVersion(aid);
      final out = await merger(
        run: answering(
          Merged(
            files: {'index.html': utf('<h1>Plants</h1><p>mine</p>')},
            title: 'Plant log',
            capabilities: const {
              'db': {},
              'net': {
                'hosts': ['api.example'],
              },
            },
            note: 'Both.',
          ),
        ),
      ).bringIntoMine(aid, v2, confirm: (_) async => true);
      expect(out, isA<UpdateRefused>());
      expect(
        (out as UpdateRefused).message,
        'Claude’s combined version asks for capabilities neither version '
        'has, so Clax didn’t take it.',
      );
      expect(me.api.currentVersion(aid), before);
      expect(await live(aid), '<p>v1</p><p>mine</p>');
    });

    test('what the merge takes from vN beyond mine is on the offer', () async {
      final sampling = await BundleReader.read(
        (await makeBundle(
          lineage: lineageA,
          version: 2,
          files: {'index.html': '<h1>Plants</h1><p>v1</p>'},
          capabilities: const {'db': {}, 'sample': {}},
        )).bytes,
      );
      Map<String, Object?>? told;
      Merged? offered;
      final out = await merger(
        run: answering(
          Merged(
            files: {'index.html': utf('<h1>Plants</h1><p>mine</p>')},
            title: 'Plant log',
            capabilities: const {'db': {}, 'sample': {}},
            note: 'Both.',
          ),
          saw: (_, _, _, caps) => told = caps,
        ),
      ).bringIntoMine(
        aid,
        sampling,
        confirm: (m) async {
          offered = m;
          return false;
        },
      );
      expect(out, isA<UpdateCancelled>(), reason: _say(out));
      // The agent was told v2's declaration; the offer names what it adds.
      expect(told, {'db': {}, 'sample': {}});
      expect(offered!.added.capabilities, ['sample']);
      expect(offered!.added.hosts, isEmpty);
    });

    test('without an agent nothing merges', () async {
      final m = merger();
      expect(m.canMerge, isFalse);
      expect(await m.bringIntoMine(aid, v2), isA<UpdateRefused>());
      expect(await live(aid), '<p>v1</p><p>mine</p>');
    });
  });

  group('Diverged', () {
    late String aid;
    late FakePublicCopy copy;
    const sam = PublishedBy(name: 'Sam', login: 'sam', publicId: 'p_sam');

    /// An app made here, v1 published (the base), v2 made here since, and
    /// v2 published on the web by Sam: both moved.
    setUp(() async {
      final made = await me.api.createArtifact(
        PublishRequest(
          title: 'Plant log',
          icon: 'leaf',
          entries: {'index.html': FileEntry.text('<p>v1</p>')},
        ),
      );
      aid = made.artifact.id;
      await me.api.publish(
        aid,
        PublishRequest(ifVersion: 1, files: {'index.html': '<p>v2 here</p>'}),
      );
      for (final v in (await me.api.getArtifact(aid)).versions) {
        remote.localVersion(
          aid,
          v.n,
          contentSha256: v.contentSha256!,
          title: 'Plant log',
        );
      }
      expect(await publisher().publish(aid, version: 1), isA<Published>());
      copy = remote.copies.values.single;
      remote.publishThere(
        copy.raid,
        contentSha256: 'sha256:web',
        by: sam,
        files: {'index.html': utf('<p>v2 on the web</p>')},
      );
      me.api.calls.clear();
      remote.calls.clear();
    });

    test('take_theirs_from_web_head_reads_files_and_publishes_locally', () async {
      final checks = me.inspector.calls.length;
      final out = await merger().resolveDiverged(
        aid,
        DivergeChoice.takeTheirs,
      );
      expect(out, isA<DivergeDone>());
      out as DivergeDone;
      expect(out.localVersion, 3);
      expect(out.remoteVersion, isNull);
      // The head's files, read through the remote and checked on a draft
      // first, are the live version; v2 made here stays in Versions.
      expect(remote.calls, contains('versionFiles'));
      expect(me.api.calls.where((c) => c.name == 'createDraft'), hasLength(1));
      expect(me.inspector.calls.length, greaterThan(checks));
      expect(await me.api.listDrafts(), isEmpty);
      expect(await live(aid), '<p>v2 on the web</p>');
      expect(utf8.decode((await me.api.readFileBytes(aid, 2, 'index.html'))!),
          '<p>v2 here</p>');
      final v3 = await version(aid, 3);
      expect(v3.label, 'v2 from clax.example');
      expect(v3.note, contains('by @sam'));
      // Nothing was published to the remote; the link's base is the head,
      // here and in the core, and the taken files count as unmodified.
      expect(copy.versions, hasLength(2));
      final e = (await me.store.entry(aid))!;
      expect(e.link!.baseRemoteVersion, 2);
      // Sam's code is live now: the next change asks about Sam's account.
      expect(e.liveFrom?.key, 'account:https://clax.example/p_sam');
      expect(e.liveFrom?.who, '@sam');
      expect(remote.links[aid]!.remoteN, 2);
      expect(
        e.isModified(
          contentSha256: v3.contentSha256!,
          capabilities: (await me.api.getArtifact(aid)).artifact.capabilities,
        ),
        isFalse,
      );
    });

    test('a version read the remote fails reads in the person’s words, the '
        'raw text kept as details', () async {
      remote.failNext(
        'versionFiles',
        const ClaxApiError(status: 500, code: 'internal', message: 'boom'),
      );
      var out = await merger().resolveDiverged(aid, DivergeChoice.takeTheirs);
      expect(out, isA<DivergeFailed>(), reason: _said(out));
      expect(
        (out as DivergeFailed).message,
        'Couldn’t get this app’s versions from clax.example.',
      );
      expect(out.details, ['boom']);

      remote.tooOld.add('https://clax.example');
      out = await merger().resolveDiverged(aid, DivergeChoice.takeTheirs);
      expect(
        (out as DivergeFailed).message,
        'clax.example needs an update before the Clax app can combine '
        'versions.',
      );
      expect(me.api.currentVersion(aid), 2);
    });

    test('a web head whose files fail their check leaves the app', () async {
      me.inspector.fail(const InspectorError('page threw: boom'));
      final out = await merger().resolveDiverged(
        aid,
        DivergeChoice.takeTheirs,
      );
      expect(out, isA<DivergeNotTaken>());
      expect((out as DivergeNotTaken).outcome, isA<UpdateCheckFailed>());
      expect(me.api.currentVersion(aid), 2);
      expect((await me.store.entry(aid))!.link!.baseRemoteVersion, 1);
    });

    test('combine_publishes_head_plus_one', () async {
      late Map<String, Uint8List> sawMine, sawTheirs;
      late String sawFrom;
      late VersionSource sawSource;
      // What the push of v3 sends: the combined files' hash (the same
      // files make the same hash in any app).
      final twin = await me.api.createArtifact(
        PublishRequest(
          title: 'Plant log',
          entries: {'index.html': FileEntry.bytes(utf('<p>both</p>'))},
        ),
      );
      remote.localVersion(
        aid,
        3,
        contentSha256: twin.version.contentSha256!,
        title: 'Plant log',
      );
      var asked = false;
      final out = await merger(
        run:
            (
              aid, {
              required mine,
              required theirs,
              required base,
              required from,
              required theirsCapabilities,
              required source,
              cancelled,
            }) async {
              sawMine = mine;
              sawTheirs = theirs;
              sawFrom = from;
              sawSource = source;
              return Merged(
                files: {'index.html': utf('<p>both</p>')},
                title: 'Plant log',
                capabilities: const {},
                note: 'Took the web heading and kept the list here.',
              );
            },
      ).resolveDiverged(
        aid,
        DivergeChoice.combine,
        confirm: (_) async => asked = true,
      );
      expect(out, isA<DivergeDone>(), reason: _said(out));
      out as DivergeDone;
      expect(asked, isTrue);
      expect(utf8.decode(sawMine['index.html']!), '<p>v2 here</p>');
      expect(utf8.decode(sawTheirs['index.html']!), '<p>v2 on the web</p>');
      expect(sawFrom, 'v2 by @sam on clax.example');
      // The head's files are the account's that published them.
      expect(sawSource.key, 'account:https://clax.example/p_sam');
      expect(sawSource.who, '@sam');
      expect(sawSource.origin, 'https://clax.example');
      expect(sawSource.account?.login, 'sam');
      // Checked, taken as v3 naming both sources, then published as the
      // head (v2) + 1.
      expect(out.localVersion, 3);
      expect(out.remoteVersion, 3);
      expect(copy.versions.last.n, 3);
      expect(copy.versions.last.bundle, isNotNull);
      final v3 = await version(aid, 3);
      expect(
        v3.note,
        'From v2 by @sam on clax.example and v2 here: Took the web heading '
        'and kept the list here.',
      );
      expect(await live(aid), '<p>both</p>');
      final l = (await me.store.entry(aid))!.link!;
      expect(l.baseRemoteVersion, 3);
      expect(l.published.last, 3);
    });

    test('a core that refuses the re-base fails the combine and moves '
        'neither base', () async {
      final twin = await me.api.createArtifact(
        PublishRequest(
          title: 'Plant log',
          entries: {'index.html': FileEntry.bytes(utf('<p>both</p>'))},
        ),
      );
      remote.localVersion(
        aid,
        3,
        contentSha256: twin.version.contentSha256!,
        title: 'Plant log',
      );
      remote.failNext(
        'link',
        const ClaxApiError(status: 400, code: 'refused', message: 'no link'),
      );
      final out = await merger(
        run: answering(
          Merged(
            files: {'index.html': utf('<p>both</p>')},
            title: 'Plant log',
            capabilities: const {},
            note: 'Both.',
          ),
        ),
      ).resolveDiverged(aid, DivergeChoice.combine, confirm: (_) async => true);
      expect(out, isA<DivergeFailed>(), reason: _said(out));
      expect(
        (out as DivergeFailed).message,
        'v3 is here, but Clax couldn’t record it as based on '
        'clax.example’s v2, so nothing was published.',
      );
      expect(out.details, ['no link']);
      // Neither base moved, and nothing was published.
      expect((await me.store.entry(aid))!.link!.baseRemoteVersion, 1);
      expect(remote.links[aid]!.remoteN, 1);
      expect(copy.versions, hasLength(2));
      expect(remote.calls, isNot(contains('publish')));
    });

    test('a core that refuses the re-base fails Take theirs and moves '
        'neither base', () async {
      remote.failNext(
        'link',
        const ClaxApiError(status: 400, code: 'refused', message: 'no link'),
      );
      final out = await merger().resolveDiverged(
        aid,
        DivergeChoice.takeTheirs,
      );
      expect(out, isA<DivergeFailed>(), reason: _said(out));
      expect((out as DivergeFailed).message, startsWith('v3 is here, but'));
      final e = (await me.store.entry(aid))!;
      expect(e.link!.baseRemoteVersion, 1);
      expect(remote.links[aid]!.remoteN, 1);
    });

    test('combine gives the agent the head’s capabilities and refuses '
        'what neither declares', () async {
      copy.capabilities = {'sample': {}};
      Map<String, Object?>? told;
      String? from;
      Merged? offered;
      final out = await merger(
        run: answering(
          Merged(
            files: {'index.html': utf('<p>both</p>')},
            title: 'Plant log',
            capabilities: const {'sample': {}},
            note: 'Both.',
          ),
          saw: (_, _, f, caps) {
            from = f;
            told = caps;
          },
        ),
      ).resolveDiverged(
        aid,
        DivergeChoice.combine,
        confirm: (m) async {
          offered = m;
          return false;
        },
      );
      expect(out, isA<DivergeNotTaken>(), reason: _said(out));
      expect(from, 'v2 by @sam on clax.example');
      expect(told, {'sample': {}});
      expect(offered!.added.capabilities, ['sample']);

      final refused = await merger(
        run: answering(
          Merged(
            files: {'index.html': utf('<p>both</p>')},
            title: 'Plant log',
            capabilities: const {'sample': {}, 'db': {}},
            note: 'Both.',
          ),
        ),
      ).resolveDiverged(aid, DivergeChoice.combine, confirm: (_) async => true);
      expect(refused, isA<DivergeFailed>());
      expect(
        (refused as DivergeFailed).message,
        startsWith('Claude’s combined version asks for capabilities'),
      );
      expect(me.api.currentVersion(aid), 2);
    });

    test('a combine discarded, failing its check or cancelled leaves the '
        'app, its link and the public copy', () async {
      final merged = Merged(
        files: {'index.html': utf('<p>both</p>')},
        title: 'Plant log',
        capabilities: const {},
        note: 'Both.',
      );
      Future<void> unchanged(DivergeOutcome out) async {
        expect(out, isA<DivergeNotTaken>(), reason: _said(out));
        expect(me.api.currentVersion(aid), 2);
        expect(await live(aid), '<p>v2 here</p>');
        expect((await me.store.entry(aid))!.link!.baseRemoteVersion, 1);
        expect(remote.links[aid]!.remoteN, 1);
        expect(copy.versions, hasLength(2));
        expect(remote.calls, isNot(contains('publish')));
        expect(remote.calls, isNot(contains('link')));
      }

      // Discard on the offer.
      var out = await merger(run: answering(merged)).resolveDiverged(
        aid,
        DivergeChoice.combine,
        confirm: (_) async => false,
      );
      await unchanged(out);
      expect((out as DivergeNotTaken).outcome, isA<UpdateCancelled>());

      // A check that fails.
      me.inspector.fail(const InspectorError('page threw: boom'));
      out = await merger(run: answering(merged)).resolveDiverged(
        aid,
        DivergeChoice.combine,
        confirm: (_) async => true,
      );
      await unchanged(out);
      expect((out as DivergeNotTaken).outcome, isA<UpdateCheckFailed>());

      // The merge cancelled.
      out = await merger(
        run: answering(const MergeFailed('Stopped', cancelled: true)),
      ).resolveDiverged(aid, DivergeChoice.combine);
      await unchanged(out);
      expect((out as DivergeNotTaken).outcome, isA<UpdateCancelled>());
    });

    test('new_app_relinks_with_forked_from', () async {
      final was = (await me.store.entry(aid))!;
      final out = await merger().resolveDiverged(aid, DivergeChoice.newApp);
      expect(out, isA<DivergeDone>());
      out as DivergeDone;
      // The core's link to the old copy ended before the push made a new
      // copy; the old copy is as it was.
      expect(
        remote.calls.where((c) => c == 'unlink' || c == 'publish').toList(),
        ['unlink', 'publish'],
      );
      expect(remote.copies, hasLength(2));
      expect(copy.versions, hasLength(2));
      final fresh = remote.copies.values.firstWhere((c) => c != copy);
      expect(out.url, fresh.url);
      expect(out.remoteVersion, 1);
      // The push named the old copy as the one it was made from.
      expect(fresh.forkedFrom, copy.raid);
      // A new lineage, forked from the one the app had.
      final b = await BundleReader.read(fresh.versions.single.bundle!.bytes);
      expect(b.manifest.lineage, isNot(was.lineage));
      expect(b.manifest.version, 1);
      expect(b.manifest.forkedFrom?.lineage, was.lineage);
      // The app is relinked to it, here and in the core.
      final e = (await me.store.entry(aid))!;
      expect(e.lineage, b.manifest.lineage);
      expect(e.link!.remoteArtifactId, fresh.raid);
      expect(e.link!.baseRemoteVersion, 1);
      expect(e.link!.published, [2]);
      expect(e.upstream?.lineage, was.lineage);
      expect(remote.links[aid]!.raid, fresh.raid);
    });

    test('a new app whose push fails links the old copy again', () async {
      remote.failNext(
        'publish',
        const ClaxApiError(status: 400, code: 'refused', message: 'refused'),
      );
      final out = await merger().resolveDiverged(aid, DivergeChoice.newApp);
      expect(out, isA<DivergeFailed>());
      expect(remote.links[aid], (rid: r.id, raid: copy.raid, remoteN: 1));
      expect((await me.store.entry(aid))!.link!.remoteArtifactId, copy.raid);
    });
  });
}

String _say(UpdateOutcome o) => switch (o) {
  UpdateRefused(:final message, :final details) => '$message $details',
  UpdateCheckFailed(:final reason, :final details) => '$reason $details',
  _ => '$o',
};

String _said(DivergeOutcome o) => switch (o) {
  DivergeFailed(:final message, :final details) => '$message $details',
  DivergeNotTaken(:final outcome) => _say(outcome),
  _ => '$o',
};
