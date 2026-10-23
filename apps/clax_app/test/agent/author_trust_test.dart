import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:clax_app/agent/author_trust.dart';
import 'package:clax_app/agent/loop.dart';
import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/remote/models.dart'
    show PublishedBy, Remote, VersionSource;
import 'package:clax_app/ui/author_trust_sheet.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'loop_support.dart';

const _sam = 'SSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSS';
const _ed = 'EEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEE';
const _own = 'OOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOO';

LineageEntry _entry(
  String title, {
  LineageRole role = LineageRole.installed,
  String key = _sam,
  String? name = 'Sam',
  LineageUpstream? upstream,
}) => LineageEntry(
  lineage: 'l_aaaaaaaaaaaaaaaaaaaaaaaaaa',
  role: role,
  key: key,
  name: name,
  version: 1,
  released: const [],
  capabilities: const {},
  title: title,
  upstream: upstream,
  fromBundle: role == LineageRole.installed || upstream != null,
);

void main() {
  late Directory dir;
  late SettingsStore settings;
  late LineageStore lineages;
  late AuthorTrust trust;

  setUp(() {
    dir = Directory.systemTemp.createTempSync('clax-author-trust-');
    settings = SettingsStore(File('${dir.path}/settings.json'));
    lineages = LineageStore(dir);
    trust = AuthorTrust(
      lineages: lineages,
      settings: settings,
      ownKey: () async => _own,
      ownAccount: (origin, by) async => isOwnAccount(
        const [
          Remote(
            id: 'r1',
            origin: 'https://clax.example',
            name: 'clax.example',
            status: 'connected',
            personName: 'Me',
            personPublicId: 'p_me',
          ),
        ],
        origin,
        by,
      ),
    );
  });
  tearDown(() => dir.deleteSync(recursive: true));

  /// The apps: the person's own two, Sam's, a fork of Sam's, and a fork of
  /// an app installed nowhere here.
  Future<void> seed() async {
    await lineages.put(
      'a_mine',
      _entry('Mine', role: LineageRole.author, key: _own),
    );
    await lineages.put('a_self', _entry('Self', key: _own, name: 'Me'));
    await lineages.put('a_sam', _entry('Plant log'));
    await lineages.put(
      'a_fork',
      _entry(
        'Plant log (mine)',
        role: LineageRole.author,
        key: _own,
        name: 'Me',
        upstream: const LineageUpstream(
          lineage: 'l_bbbbbbbbbbbbbbbbbbbbbbbbbb',
          key: _sam,
        ),
      ),
    );
    await lineages.put(
      'a_far',
      _entry(
        'Far',
        role: LineageRole.author,
        key: _own,
        upstream: const LineageUpstream(
          lineage: 'l_cccccccccccccccccccccccccc',
          key: _ed,
        ),
      ),
    );
  }

  Future<BuildContext> page(WidgetTester tester, Brightness b) async {
    await tester.runAsync(seed);
    late BuildContext context;
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(b, TargetPlatform.iOS),
        home: Builder(
          builder: (c) {
            context = c;
            return const Scaffold(body: Text('Home'));
          },
        ),
      ),
    );
    return context;
  }

  final samSheet = find.text(authorTrustTitle('Sam'));

  test('only an app someone else wrote has an author to trust', () async {
    await seed();
    expect(await trust.authorOf('a_mine'), isNull);
    expect(await trust.authorOf('a_self'), isNull);
    expect(await trust.authorOf('a_none'), isNull);
    expect(
      await trust.authorOf('a_sam'),
      (key: _sam, who: 'Sam', title: 'Plant log'),
    );
    // A fork's author is its original's, named by the app installed from it.
    expect(
      await trust.authorOf('a_fork'),
      (key: _sam, who: 'Sam', title: 'Plant log (mine)'),
    );
    // An original installed nowhere here is named by its key's fingerprint.
    final far = (await trust.authorOf('a_far'))!;
    expect(far.key, _ed);
    expect(far.who, matches(RegExp(r'^[A-Z0-9]{4}(-[A-Z0-9]{4}){3}$')));
  });

  test('an app whose lineage cannot be read has an unknown author, never '
      'the person', () async {
    await seed();
    // An entry that does not parse.
    final raw = jsonDecode(lineages.file.readAsStringSync()) as Map;
    raw['a_bad'] = {'role': 42};
    lineages.file.writeAsStringSync(jsonEncode(raw));
    expect(await trust.authorOf('a_bad'), AuthorTrust.unknown);
    expect(await trust.ensure(null, 'a_bad'), isFalse);
    // An app with no entry is still one made here.
    expect(await trust.authorOf('a_none'), isNull);
    expect(await trust.ensure(null, 'a_none'), isTrue);

    // A damaged file: every app's author is unknown, and stays so once the
    // file was set aside and a new one written.
    lineages.file.writeAsStringSync('not json');
    expect(await trust.authorOf('a_sam'), AuthorTrust.unknown);
    expect(await trust.authorOf('a_none'), AuthorTrust.unknown);
    expect(await trust.ensure(null, 'a_none'), isFalse);
    await lineages.put('a_new', _entry('New', role: LineageRole.author));
    expect(lineages.damaged, isFalse);
    expect(await trust.authorOf('a_sam'), AuthorTrust.unknown);
    expect(
      await LineageStore(dir).entry('a_sam'),
      isNull,
      reason: 'another process after the set-aside',
    );
    expect(await AuthorTrust(
      lineages: LineageStore(dir),
      settings: settings,
    ).authorOf('a_sam'), AuthorTrust.unknown);
  });

  testWidgets('the person’s own apps go without asking', (tester) async {
    final context = await page(tester, Brightness.light);
    for (final aid in ['a_mine', 'a_self']) {
      expect(await tester.runAsync(() => trust.ensure(context, aid)), isTrue);
      await tester.pumpAndSettle();
      expect(find.byType(AuthorTrustBody), findsNothing);
    }
  });

  testWidgets('the first ask shows once; Continue is kept for the app and '
      'its author, so the next does not ask', (tester) async {
    final context = await page(tester, Brightness.light);
    bool? answer;
    unawaited(trust.ensure(context, 'a_sam').then((a) => answer = a));
    await tester.pumpAndSettle();
    expect(samSheet, findsOneWidget);
    expect(find.text(authorTrustBody('Plant log', 'Sam')), findsOneWidget);
    expect(answer, isNull);

    await tester.tap(find.text('Continue'));
    await tester.pumpAndSettle();
    expect(answer, isTrue);
    expect((await settings.load()).trustedAuthors, {
      'a_sam': [_sam],
    });

    expect(await trust.ensure(context, 'a_sam'), isTrue);
    await tester.pumpAndSettle();
    expect(samSheet, findsNothing);
  });

  testWidgets('Cancel answers false and keeps nothing; the next ask shows '
      'again', (tester) async {
    final context = await page(tester, Brightness.dark);
    bool? answer;
    unawaited(trust.ensure(context, 'a_sam').then((a) => answer = a));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
    expect(answer, isFalse);
    expect((await settings.load()).trustedAuthors, isEmpty);

    unawaited(trust.ensure(context, 'a_sam'));
    await tester.pumpAndSettle();
    expect(samSheet, findsOneWidget);
  });

  testWidgets('runs that ask while the sheet shows share it and its answer', (
    tester,
  ) async {
    final context = await page(tester, Brightness.light);
    final answers = <bool>[];
    unawaited(trust.ensure(context, 'a_sam').then(answers.add));
    await tester.pumpAndSettle();
    unawaited(trust.ensure(context, 'a_sam').then(answers.add));
    await tester.pumpAndSettle();
    expect(samSheet, findsOneWidget);
    await tester.tap(find.text('Continue'));
    await tester.pumpAndSettle();
    expect(answers, [true, true]);
  });

  testWidgets('a new author asks again', (tester) async {
    final context = await page(tester, Brightness.light);
    await settings.save(
      AppSettings(
        trustedAuthors: {
          'a_sam': [_sam],
        },
      ),
    );
    await tester.runAsync(
      () => lineages.put('a_sam', _entry('Plant log', key: _ed, name: 'Ed')),
    );
    unawaited(trust.ensure(context, 'a_sam'));
    await tester.pumpAndSettle();
    expect(find.text(authorTrustTitle('Ed')), findsOneWidget);
  });

  testWidgets('with no screen to ask over, the answer is no', (tester) async {
    await tester.runAsync(seed);
    expect(await trust.ensure(null, 'a_sam'), isFalse);
    expect(await trust.ensure(null, 'a_mine'), isTrue);
  });

  testWidgets('a change refused at the sheet sends nothing to Claude', (
    tester,
  ) async {
    final context = await page(tester, Brightness.light);
    final api = FakeClaxApi();
    final sid = (await api.registerAppSession(installId: 'i', pid: 1)).id;
    final aid = api.seedArtifact('Plant log', {
      'index.html':
          '<!doctype html><title>Plant log</title><button>Save</button>',
    });
    await tester.runAsync(() => lineages.put(aid, _entry('Plant log')));
    final backend = HeldBackend([
      change(reply: 'Keep.', edits: [edit('Save', 'Keep')]),
    ]);
    final loop = AgentLoop(
      api: api,
      singleCall: () => backend,
      prompts: PromptBuilder(rootBundle),
      model: () => 'claude-haiku-5-5',
      timings: RecordingTimings(),
      gate: (aid, {required build}) => trust.ensure(context, aid),
    );
    final item = api.sendComment(aid, 'Say Keep', queue: false);
    ChangeOutcome? out;
    unawaited(loop.change(aid, [item], session: sid).then((o) => out = o));
    await tester.pumpAndSettle();
    expect(samSheet, findsOneWidget);
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
    expect(out, isA<ChangeCancelled>());
    expect(backend.seen, isEmpty);
  });

  group('a merge asks about the author of the version it brings in', () {
    final ed = VersionSource.account(
      'https://clax.example',
      const PublishedBy(name: 'Ed', login: 'ed', publicId: 'p_ed'),
    );

    testWidgets('once per app and source, naming the source', (tester) async {
      final context = await page(tester, Brightness.light);
      bool? answer;
      unawaited(
        trust
            .ensureSource(context, 'a_mine', ed, title: 'Mine')
            .then((a) => answer = a),
      );
      await tester.pumpAndSettle();
      expect(find.text(authorTrustTitle('@ed')), findsOneWidget);
      expect(find.text(authorTrustBody('Mine', '@ed')), findsOneWidget);
      await tester.tap(find.text('Continue'));
      await tester.pumpAndSettle();
      expect(answer, isTrue);
      expect(
        await trust.ensureSource(context, 'a_mine', ed, title: 'Mine'),
        isTrue,
      );
      await tester.pumpAndSettle();
      expect(find.byType(AuthorTrustBody), findsNothing);

      // Another source for the same app asks again; so does Ed's version
      // for another app.
      unawaited(
        trust.ensureSource(
          context,
          'a_mine',
          const VersionSource(key: _sam, who: 'Sam'),
          title: 'Mine',
        ),
      );
      await tester.pumpAndSettle();
      expect(samSheet, findsOneWidget);
      await tester.tap(find.text('Cancel'));
      await tester.pumpAndSettle();
      unawaited(trust.ensureSource(context, 'a_sam', ed, title: 'Plant log'));
      await tester.pumpAndSettle();
      expect(find.text(authorTrustTitle('@ed')), findsOneWidget);
    });

    testWidgets('the person’s own versions never ask', (tester) async {
      final context = await page(tester, Brightness.light);
      // Signed here, or published by their account from another device.
      for (final own in const [
        VersionSource(key: _own, who: 'Me'),
        VersionSource(
          key: 'account:https://clax.example/p_me',
          who: '@me',
          origin: 'https://clax.example',
          account: PublishedBy(name: 'Me', login: 'me', publicId: 'p_me'),
        ),
      ]) {
        expect(
          await trust.ensureSource(context, 'a_mine', own, title: 'Mine'),
          isTrue,
        );
        await tester.pumpAndSettle();
        expect(find.byType(AuthorTrustBody), findsNothing);
      }
    });
  });

  testWidgets('an account named as the person but with another public ID '
      'asks, and so does one with no ID', (tester) async {
    final context = await page(tester, Brightness.light);
    for (final (by, n) in const [
      (PublishedBy(name: 'Me', login: 'me', publicId: 'p_mallory'), 1),
      (PublishedBy(name: 'Me', login: 'me'), 2),
    ]) {
      unawaited(
        trust.ensureSource(
          context,
          'a_mine',
          VersionSource.account('https://clax.example', by),
          title: 'Mine',
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text(authorTrustTitle('@me')), findsOneWidget, reason: '$n');
      await tester.tap(find.text('Cancel'));
      await tester.pumpAndSettle();
    }
  });

  test('only the same public ID is the person’s own account', () {
    const remotes = [
      Remote(
        id: 'r1',
        origin: 'https://clax.example',
        name: 'clax.example',
        status: 'connected',
        personName: 'Me',
        personPublicId: 'p_me',
      ),
    ];
    bool own(PublishedBy by, [String origin = 'https://clax.example']) =>
        isOwnAccount(remotes, origin, by);
    expect(own(const PublishedBy(name: 'Someone', publicId: 'p_me')), isTrue);
    expect(own(const PublishedBy(name: 'Me', publicId: 'p_other')), isFalse);
    expect(own(const PublishedBy(name: 'Me')), isFalse);
    expect(
      own(const PublishedBy(name: 'Me', publicId: 'p_me'), 'https://x.example'),
      isFalse,
    );
    // Keys never let a name stand for an ID.
    expect(
      VersionSource.account(
        'https://clax.example',
        const PublishedBy(name: 'p_me'),
      ).key,
      isNot(
        VersionSource.account(
          'https://clax.example',
          const PublishedBy(name: 'Me', publicId: 'p_me'),
        ).key,
      ),
    );
  });

  testWidgets('code taken into an app made here asks about who published '
      'it, once', (tester) async {
    final context = await page(tester, Brightness.light);
    await tester.runAsync(
      () => lineages.update(
        'a_mine',
        (e) async => e!.copyWith(
          liveFrom: () => const VersionSource(key: _ed, who: 'Ed'),
        ),
      ),
    );
    bool? answer;
    unawaited(trust.ensure(context, 'a_mine').then((a) => answer = a));
    await tester.pumpAndSettle();
    expect(find.text(authorTrustTitle('Ed')), findsOneWidget);
    expect(find.text(authorTrustBody('Mine', 'Ed')), findsOneWidget);
    await tester.tap(find.text('Continue'));
    await tester.pumpAndSettle();
    expect(answer, isTrue);
    expect(await trust.ensure(context, 'a_mine'), isTrue);
    await tester.pumpAndSettle();
    expect(find.byType(AuthorTrustBody), findsNothing);
  });

  test('copy for the sheet', () {
    expect(authorTrustTitle('Sam'), 'Claude reads what Sam wrote');
    expect(
      authorTrustBody('Plant log', 'Sam'),
      'Plant log’s code and content were written by Sam. Claude reads all '
      'of it to make your change, and what an app says can sway what Claude '
      'does. Continue only if you trust Sam.',
    );
  });
}
