import 'dart:async';
import 'dart:io';

import 'package:clax_app/clax/api.dart';
import 'package:clax_app/home/home_host.dart';
import 'package:clax_app/home/home_settings.dart';
import 'package:clax_app/platform/device/mac_share.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/remote/fake_remote_api.dart';
import 'package:clax_app/remote/remote_api.dart' show ShareLogin;
import 'package:clax_app/remote/models.dart';
import 'package:clax_app/remote/publisher.dart';
import 'package:clax_app/remote/puller.dart';
import 'package:clax_app/ui/app_inspector.dart';
import 'package:clax_app/ui/launcher.dart';
import 'package:clax_app/ui/make_service.dart';
import 'package:clax_app/ui/remote/remote_flows.dart';
import 'package:clax_app/ui/remote/share_sheet.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../bundle/services_support.dart';
import '../support/fake_clock.dart' show FakeClock;
import '../support/fake_voice.dart';
import '../support/settle.dart';

/// The core's answer for a remote it cannot reach.
const _offline = ClaxApiError(
  status: 502,
  code: 'remote_unreachable',
  message: 'https://clax.example could not be reached',
);

/// A remote whose link change lands but whose answer is lost on the way
/// back, as when the connection drops mid-call.
class _LostAnswerRemote extends FakeRemoteApi {
  @override
  Future<void> setLink(
    String rid,
    String raid,
    String? access, {
    bool listed = false,
  }) async {
    await super.setLink(rid, raid, access, listed: listed);
    throw _offline;
  }
}

/// A remote with a public gallery.
class _GalleryRemote extends FakeRemoteApi {
  @override
  Future<Sharing> sharing(String rid, String raid) async {
    final s = await super.sharing(rid, raid);
    return Sharing(
      artifactId: s.artifactId,
      canShare: s.canShare,
      general: s.general,
      publicGallery: true,
      linkAccess: s.linkAccess,
      url: s.url,
      people: s.people,
      suggestions: s.suggestions,
      comments: s.comments,
    );
  }
}

/// A remote that suggests Mira, who joined by invite and has no GitHub
/// login, and whose general access reports no public gallery.
class _PeopleRemote extends FakeRemoteApi {
  _PeopleRemote() {
    personNames['p_mira'] = 'Mira Okafor';
  }

  @override
  Future<Sharing> sharing(String rid, String raid) async {
    final s = await super.sharing(rid, raid);
    return Sharing(
      artifactId: s.artifactId,
      canShare: s.canShare,
      general: s.general,
      linkAccess: s.linkAccess,
      url: s.url,
      people: s.people,
      suggestions: const [
        ShareCandidate(person: 'p_mira', name: 'Mira Okafor'),
      ],
      comments: s.comments,
    );
  }
}

void main() {
  late FakeRemoteApi remote;
  late FakePublicCopy copy;
  late List<String> copied;
  late List<String> sent;

  void setUpRemote([FakeRemoteApi? api, String general = 'restricted']) {
    remote = api ?? FakeRemoteApi();
    final r = remote.connected('https://clax.example');
    copy = remote.addCopy(
      r.id,
      contentSha256: 'c1',
      title: 'Plant log',
      access: 'owner',
      general: general,
      comments: false,
    );
    copied = [];
    sent = [];
  }

  Widget host({bool platformShare = false}) => MaterialApp(
    theme: claxTheme(Brightness.light, defaultTargetPlatform),
    home: Scaffold(
      body: Builder(
        builder: (context) => TextButton(
          onPressed: () => showShareSheet(
            context,
            remote,
            rid: copy.rid,
            raid: copy.raid,
            title: 'Plant log',
            host: 'clax.example',
            copyText: (t) async => copied.add(t),
            shareLink: platformShare
                ? null
                : (url, title, origin) async => sent.add(url),
          ),
          child: const Text('go'),
        ),
      ),
    ),
  );

  /// The line People with access shows while no one is added.
  final noOne = find.text(
    'No one yet. The owner and the maker can always open it.',
  );

  /// Opens the sheet and waits until it shows [shows]: the form, unless
  /// the test reads something else.
  Future<void> open(
    WidgetTester tester, {
    bool platformShare = false,
    Finder? shows,
  }) async {
    tester.view.physicalSize = const Size(800, 2000);
    tester.view.devicePixelRatio = 2;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(host(platformShare: platformShare));
    await tester.tap(find.text('go'));
    await settle(tester, shows ?? find.byKey(const ValueKey('share-who')));
  }

  /// Picks [item] from [menu]'s menu and waits until the sheet shows
  /// [shows], what the pick leads to.
  Future<void> pick(
    WidgetTester tester,
    Key menu,
    String item, {
    required Finder shows,
  }) async {
    await tester.ensureVisible(find.byKey(menu));
    await tester.tap(find.byKey(menu));
    await settle(tester, find.widgetWithText(MenuItemButton, item));
    await tester.tap(find.text(item).last);
    await settle(tester, shows);
  }

  /// [text] in the sheet itself, not in a menu over it.
  Finder inSheet(String text) =>
      find.descendant(of: find.byType(ShareSheet), matching: find.text(text));

  /// [text] on the button of [key]'s picker.
  Finder picked(Key key, String text) =>
      find.descendant(of: find.byKey(key), matching: find.text(text));

  VoidCallback? pressedOf(WidgetTester tester, Key key) {
    final w = tester.widget(
      find.descendant(
        of: find.byKey(key),
        matching: find.byWidgetPredicate((w) => w is ButtonStyleButton),
      ),
    );
    return (w as ButtonStyleButton).onPressed;
  }

  testWidgets('share_sheet_adds_person_and_changes_access', (tester) async {
    setUpRemote();
    await open(tester);
    expect(find.text('Share Plant log'), findsOneWidget);
    expect(find.text('No one yet. The owner and the maker can always open it.'),
        findsOneWidget);

    await tester.enterText(find.byKey(const ValueKey('share-who')), '@JunP');
    await pick(
      tester,
      const ValueKey('share-add-access'),
      'Can comment',
      shows: picked(const ValueKey('share-add-access'), 'Can comment'),
    );
    await tester.tap(find.byKey(const ValueKey('share-add')));
    await settle(tester, find.text('@junp'));

    expect(copy.shares.single.githubLogin, 'junp');
    expect(copy.shares.single.access, 'commenter');
    expect(find.text('@junp'), findsOneWidget);
    expect(
      find.text('Not signed in yet: has access once they sign in with GitHub'),
      findsOneWidget,
    );
    final sid = copy.shares.single.id;
    expect(
      find.descendant(
        of: find.byKey(ValueKey('share-access-$sid')),
        matching: find.text('Can comment'),
      ),
      findsOneWidget,
    );
    // The field is cleared once the add lands.
    expect(
      tester.widget<EditableText>(find.byType(EditableText)).controller.text,
      '',
    );

    await pick(
      tester,
      ValueKey('share-access-$sid'),
      'Can edit',
      shows: inSheet('Can edit'),
    );
    expect(copy.shares.single.access, 'editor');
    final sid2 = copy.shares.single.id;
    expect(
      find.descendant(
        of: find.byKey(ValueKey('share-access-$sid2')),
        matching: find.text('Can edit'),
      ),
      findsOneWidget,
    );

    // General access: no Public without a public gallery.
    await tester.tap(find.byKey(const ValueKey('share-general')));
    await settle(tester, find.byKey(const ValueKey('link')));
    expect(find.text('Public'), findsNothing);
    await tester.tap(find.text('Anyone with the link').last);
    await settle(
      tester,
      picked(const ValueKey('share-general'), 'Anyone with the link'),
    );
    expect(copy.general, 'link');
    expect(copy.linkAccess, 'viewer');
    expect(
      find.text('Anyone with the link can open it, without signing in.'),
      findsOneWidget,
    );
    await pick(
      tester,
      const ValueKey('share-link-access'),
      'Can comment',
      shows: picked(const ValueKey('share-link-access'), 'Can comment'),
    );
    expect(copy.linkAccess, 'commenter');

    await pick(
      tester,
      ValueKey('share-access-$sid2'),
      'Remove',
      shows: noOne,
    );
    expect(copy.shares, isEmpty);
    expect(find.text('@junp'), findsNothing);
  });

  testWidgets('at large text a person\'s access picker goes under their '
      'name, so a long login keeps the row\'s width', (tester) async {
    tester.platformDispatcher.textScaleFactorTestValue = 2;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    setUpRemote();
    await remote.share(
      copy.rid,
      copy.raid,
      ShareLogin('alexandra-longname'),
      'commenter',
    );
    await open(tester);
    final sid = copy.shares.single.id;
    final name = find.text('@alexandra-longname');
    await tester.ensureVisible(name);
    expect(
      tester.getTopLeft(find.byKey(ValueKey('share-access-$sid'))).dy,
      greaterThan(tester.getBottomLeft(name).dy),
    );
  });

  testWidgets('each access picker tells a screen reader what it sets', (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    setUpRemote(null, 'link');
    await open(tester);
    for (final (key, label) in [
      ('share-add-access', 'Access for the person you add'),
      ('share-general', 'General access'),
      ('share-link-access', 'Access for anyone with the link'),
    ]) {
      expect(
        tester.getSemantics(find.byKey(ValueKey(key))).label,
        startsWith(label),
      );
    }
    semantics.dispose();
  });

  testWidgets('a name or login that is neither refuses the add', (
    tester,
  ) async {
    setUpRemote();
    await open(tester);
    await tester.enterText(find.byKey(const ValueKey('share-who')), 'not a login');
    await tester.tap(find.byKey(const ValueKey('share-add')));
    await settle(tester, find.text(addRefusal));
    expect(
      find.text('Type a GitHub login, such as @octocat, or pick someone.'),
      findsOneWidget,
    );
    expect(remote.calls, isNot(contains('share')));
  });

  testWidgets('Public is offered where the remote has a public gallery', (
    tester,
  ) async {
    setUpRemote(_GalleryRemote());
    await open(tester);
    await pick(
      tester,
      const ValueKey('share-general'),
      'Public',
      shows: picked(const ValueKey('share-general'), 'Public'),
    );
    expect(copy.general, 'public');
    expect(
      find.text(
        "Anyone with the link can open it, and it is listed on this Clax's " // copy-check: allow
        'public page.',
      ),
      findsOneWidget,
    );
  });

  testWidgets('a suggested person with no GitHub login is added and their '
      'access changed by person', (tester) async {
    setUpRemote(_PeopleRemote());
    await open(tester);
    await tester.enterText(find.byKey(const ValueKey('share-who')), 'mir');
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('share-hint-p_mira')));
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('share-add')));
    await settleUntil(tester, () => noOne.evaluate().isEmpty);
    final added = copy.shares.single;
    expect(added.person, 'p_mira');
    expect(added.githubLogin, isNull);
    expect(added.access, 'viewer');
    expect(find.text('Mira Okafor'), findsOneWidget);

    await pick(
      tester,
      ValueKey('share-access-${added.id}'),
      'Can edit',
      shows: inSheet('Can edit'),
    );
    expect(copy.shares.single.person, 'p_mira');
    expect(copy.shares.single.access, 'editor');
  });

  testWidgets('a known name typed in full is that person, whose access '
      'then changes by person', (tester) async {
    setUpRemote(_PeopleRemote());
    await open(tester);
    await tester.enterText(
      find.byKey(const ValueKey('share-who')),
      'mira okafor',
    );
    await tester.tap(find.byKey(const ValueKey('share-add')));
    await settleUntil(tester, () => noOne.evaluate().isEmpty);
    final added = copy.shares.single;
    expect(added.person, 'p_mira');
    await pick(
      tester,
      ValueKey('share-access-${added.id}'),
      'Can comment',
      shows: inSheet('Can comment'),
    );
    expect(copy.shares.single.person, 'p_mira');
    expect(copy.shares.single.access, 'commenter');
  });

  testWidgets('a refused add shows the reason the remote gives', (tester) async {
    setUpRemote();
    await open(tester);
    const reason = 'that is this Clax’s owner, who can already do everything';
    remote.failNext(
      'share',
      const ClaxApiError(status: 400, code: 'already_owner', message: reason),
    );
    await tester.enterText(find.byKey(const ValueKey('share-who')), '@owner');
    await tester.tap(find.byKey(const ValueKey('share-add')));
    await settle(tester, find.text(reason));
    expect(find.text(reason), findsOneWidget);
    expect(copy.shares, isEmpty);
  });

  testWidgets('two taps on Add in one frame send one change', (tester) async {
    setUpRemote();
    await open(tester);
    await tester.enterText(find.byKey(const ValueKey('share-who')), '@junp');
    // Both presses land before the sheet rebuilds, as two taps or Enter
    // and Add in one frame do.
    final add = pressedOf(tester, const ValueKey('share-add'))!;
    add();
    add();
    await settleUntil(tester, () => noOne.evaluate().isEmpty);
    expect(remote.calls.where((c) => c == 'share'), hasLength(1));
    expect(copy.shares, hasLength(1));
  });

  testWidgets('a copy already Public keeps Public among the choices with '
      'no public gallery', (tester) async {
    setUpRemote(_PeopleRemote());
    copy
      ..general = 'public'
      ..linkAccess = 'viewer'
      ..listed = true;
    await open(tester);
    await tester.tap(find.byKey(const ValueKey('share-general')));
    await settle(tester, find.byKey(const ValueKey('public')));
    expect(find.byKey(const ValueKey('public')), findsOneWidget);
    expect(find.byKey(const ValueKey('link')), findsOneWidget);
    await tester.tapAt(const Offset(10, 10));
    await settleUntil(
      tester,
      () => find.byType(MenuItemButton).evaluate().isEmpty,
    );
  });

  testWidgets('comments_switch_calls_setComments', (tester) async {
    setUpRemote();
    await open(tester);
    expect(find.text('Comments'), findsOneWidget);
    expect(
      find.text('Off: nobody can comment, whatever their access.'),
      findsOneWidget,
    );
    // Above the roles: over the add field and People with access.
    expect(
      tester.getTopLeft(find.byKey(const ValueKey('share-comments'))).dy,
      lessThan(tester.getTopLeft(find.byKey(const ValueKey('share-who'))).dy),
    );
    await tester.tap(find.byKey(const ValueKey('share-comments')));
    await settle(tester, find.text('People who can comment may comment.'));
    expect(remote.calls, contains('setComments'));
    expect(copy.comments, isTrue);
    expect(find.text('People who can comment may comment.'), findsOneWidget);
  });

  testWidgets('a failed call says so and shows what the remote holds', (
    tester,
  ) async {
    setUpRemote();
    await open(tester);
    remote.failNext('setComments', _offline);
    remote.calls.clear();
    await tester.tap(find.byKey(const ValueKey('share-comments')));
    await settle(tester, find.byKey(const ValueKey('share-error')));
    expect(
      find.text(
        'Couldn’t reach clax.example. Check your connection and try again.',
      ),
      findsOneWidget,
    );
    // Read again after the failure: still off, as the remote holds it.
    expect(remote.calls, ['setComments', 'sharing']);
    expect(copy.comments, isFalse);
    expect(
      find.text('Off: nobody can comment, whatever their access.'),
      findsOneWidget,
    );

    // The next change clears the error.
    await tester.tap(find.byKey(const ValueKey('share-comments')));
    await settle(tester, find.text('People who can comment may comment.'));
    expect(find.byKey(const ValueKey('share-error')), findsNothing);
    expect(copy.comments, isTrue);
  });

  testWidgets('a change whose answer was lost shows the change that landed', (
    tester,
  ) async {
    setUpRemote(_LostAnswerRemote());
    await open(tester);
    await pick(
      tester,
      const ValueKey('share-general'),
      'Anyone with the link',
      shows: find.byKey(const ValueKey('share-error')),
    );
    expect(copy.general, 'link');
    expect(find.byKey(const ValueKey('share-error')), findsOneWidget);
    expect(
      find.text('Anyone with the link can open it, without signing in.'),
      findsOneWidget,
    );
    expect(pressedOf(tester, const ValueKey('share-copy')), isNotNull);
  });

  testWidgets('a sheet that cannot be read offers Try again', (tester) async {
    setUpRemote();
    remote.failNext('sharing', _offline);
    await open(tester, shows: find.text('Try again'));
    expect(
      find.text(
        'Couldn’t reach clax.example. Check your connection and try again.',
      ),
      findsOneWidget,
    );
    await tester.tap(find.text('Try again'));
    await settle(tester, find.byKey(const ValueKey('share-who')));
    expect(find.byKey(const ValueKey('share-who')), findsOneWidget);
  });

  testWidgets('a sheet that cannot be read says it was not read, not that a '
      'change failed', (tester) async {
    setUpRemote();
    remote.failNext(
      'sharing',
      const ClaxApiError(status: 500, code: 'internal', message: 'boom'),
    );
    await open(
      tester,
      shows: find.text('Couldn’t read who can open Plant log.'),
    );
    expect(find.text('Couldn’t read who can open Plant log.'), findsOneWidget);
    expect(find.textContaining('Couldn’t change'), findsNothing);
  });

  testWidgets('a device the remote signed out is told to connect again', (
    tester,
  ) async {
    setUpRemote();
    await open(tester);
    remote.failNext(
      'setComments',
      const ClaxApiError(
        status: 502,
        code: 'remote_signed_out',
        message: 'the remote no longer takes this device',
      ),
    );
    await tester.tap(find.byKey(const ValueKey('share-comments')));
    await settle(tester, find.byKey(const ValueKey('share-error')));
    expect(
      find.text(
        'This device is signed out of clax.example. Connect again in '
        'Settings › Sharing.',
      ),
      findsOneWidget,
    );
  });

  testWidgets('a hosted Clax too old for sharing says it needs an update', (
    tester,
  ) async {
    setUpRemote();
    remote.tooOld.add('https://clax.example');
    await open(
      tester,
      shows: find.text(
        'clax.example needs an update before the Clax app can share apps.',
      ),
    );
    expect(
      find.text(
        'clax.example needs an update before the Clax app can share apps.',
      ),
      findsOneWidget,
    );
  });

  test('links share through the Mac’s picker on the Mac, share_plus '
      'elsewhere', () {
    expect(shareLinkFor(TargetPlatform.macOS), same(shareLinkByMac));
    for (final p in [
      TargetPlatform.iOS,
      TargetPlatform.android,
      TargetPlatform.linux,
      TargetPlatform.windows,
    ]) {
      expect(shareLinkFor(p), same(shareLinkByPlugin), reason: '$p');
    }
  });

  testWidgets('on the Mac, Share link… goes to the Mac’s picker, never '
      'share_plus', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    final calls = <MethodCall>[];
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      MacShare.channel,
      (call) async {
        calls.add(call);
        return true;
      },
    );
    try {
      setUpRemote(null, 'link');
      await open(tester, platformShare: true);
      expect(pressedOf(tester, const ValueKey('share-send')), isNotNull);
      await tester.ensureVisible(find.byKey(const ValueKey('share-send')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('share-send')));
      await settleUntil(tester, () => calls.isNotEmpty);
      expect(calls.single.method, 'share');
      expect(calls.single.arguments, {'text': copy.url});
    } finally {
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        MacShare.channel,
        null,
      );
      debugDefaultTargetPlatformOverride = null;
    }
  });

  testWidgets('copy_link_disabled_while_restricted_alone', (tester) async {
    setUpRemote();
    await open(tester);
    expect(pressedOf(tester, const ValueKey('share-copy')), isNull);
    expect(pressedOf(tester, const ValueKey('share-send')), isNull);

    await pick(
      tester,
      const ValueKey('share-general'),
      'Anyone with the link',
      shows: picked(const ValueKey('share-general'), 'Anyone with the link'),
    );
    expect(pressedOf(tester, const ValueKey('share-copy')), isNotNull);
    expect(pressedOf(tester, const ValueKey('share-send')), isNotNull);
    await tester.tap(find.byKey(const ValueKey('share-copy')));
    await tester.pump();
    expect(copied, [copy.url]);
    expect(find.text('Copied'), findsOneWidget);
    await tester.pump(const Duration(seconds: 2));
    expect(find.text('Copy link'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('share-send')));
    await tester.pump();
    expect(sent, [copy.url]);

    // Restricted again, with one person added: still open to someone.
    await pick(
      tester,
      const ValueKey('share-general'),
      'Restricted',
      shows: picked(const ValueKey('share-general'), 'Restricted'),
    );
    expect(pressedOf(tester, const ValueKey('share-copy')), isNull);
    await tester.enterText(find.byKey(const ValueKey('share-who')), '@junp');
    await tester.tap(find.byKey(const ValueKey('share-add')));
    await settleUntil(tester, () => noOne.evaluate().isEmpty);
    expect(pressedOf(tester, const ValueKey('share-copy')), isNotNull);

    await tester.tap(find.byKey(const ValueKey('share-done')));
    await settleUntil(
      tester,
      () => find.byType(ShareSheet).evaluate().isEmpty,
    );
    expect(find.text('Share Plant log'), findsNothing);
  });

  testWidgets('a copy this account may not share says so', (tester) async {
    setUpRemote();
    copy.access = 'editor';
    await open(
      tester,
      shows: find.text('Only the owner and the maker of Plant log can share it.'),
    );
    expect(
      find.text('Only the owner and the maker of Plant log can share it.'),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('share-who')), findsNothing);
  });

  group('entry points', () {
    late Directory tmp;
    late Install me;
    late RemoteFlows flows;
    late String aid;

    Future<void> setUpApp(WidgetTester tester) => tester.runAsync(() async {
      tmp = Directory.systemTemp.createTempSync('share-sheet');
      me = Install(tmp);
      await me.ready();
      remote = FakeRemoteApi()..connected('https://clax.example');
      final settings = SettingsStore(File('${tmp.path}/settings.json'));
      flows = RemoteFlows(
        remote: remote,
        publisher: Publisher(
          remote: remote,
          exporter: me.exporter,
          store: me.store,
          clock: FakeClock(),
        ),
        puller: Puller(
          remote: remote,
          installer: me.installer,
          store: me.store,
          installId: () => ensureInstallId(settings),
        ),
        exporter: me.exporter,
        store: me.store,
        open: (_) {},
      );
      aid = me.api.seedArtifact('Plant log', {'index.html': '<p>hi</p>'});
      final v = await me.api.getArtifact(aid);
      remote.localVersion(
        aid,
        1,
        contentSha256: v.versions.single.contentSha256!,
        title: 'Plant log',
      );
      expect(
        await flows.publisher.publish(aid, version: 1),
        isA<Published>(),
      );
    });

    testWidgets('Share… on the inspector card opens the sharing sheet', (
      tester,
    ) async {
      await setUpApp(tester);
      addTearDown(() => tmp.deleteSync(recursive: true));
      tester.view.physicalSize = const Size(800, 2000);
      tester.view.devicePixelRatio = 2;
      addTearDown(tester.view.reset);
      Uri? browsed;
      flows = RemoteFlows(
        remote: flows.remote,
        publisher: flows.publisher,
        puller: flows.puller,
        exporter: flows.exporter,
        store: flows.store,
        open: flows.open,
        openUrl: (u) async {
          browsed = u;
          return true;
        },
      );
      await tester.pumpWidget(
        MaterialApp(
          theme: claxTheme(Brightness.light, defaultTargetPlatform),
          home: AppInspector(
            api: me.api,
            aid: aid,
            lineage: me.store.entry,
            remote: flows,
            onOpenVersion: (_, _) {},
          ),
        ),
      );
      final share = find.byKey(const ValueKey('remote-share'));
      await settle(tester, share);
      await tester.ensureVisible(share);
      await tester.tap(share);
      await settle(tester, find.text('People with access'));
      expect(find.text('Share Plant log'), findsOneWidget);
      expect(find.text('People with access'), findsOneWidget);
      expect(browsed, isNull);
    });

    testWidgets('Share… is in the menu of a linked app, and opens the '
        'sharing sheet', (tester) async {
      await setUpApp(tester);
      addTearDown(() => tmp.deleteSync(recursive: true));
      final nav = GlobalKey<NavigatorState>();
      final other = me.api.seedArtifact('Runs', {'index.html': '<p>r</p>'});
      final home = me.api.seedArtifact('Home', {'index.html': '<p>h</p>'});
      final homeFlows = HomeFlows(
        contextOf: () => nav.currentContext,
        api: me.api,
        openApp: (_) {},
        voice: FakeVoice.unsupported('No speech here'),
        remote: flows,
        lineage: me.store.entry,
      );
      final hh = HomeHost(
        api: me.api,
        stream: me.api.stream(),
        closeStream: true,
        makes: MakeService.local((r, c, s) => Completer<String>().future),
        builds: null,
        settings: HomeSettings(homeAppId: home),
        actions: homeFlows,
      );
      homeFlows.host = hh;
      addTearDown(hh.dispose);
      await tester.pumpWidget(
        MaterialApp(
          navigatorKey: nav,
          theme: claxTheme(Brightness.light, defaultTargetPlatform),
          home: const Scaffold(),
        ),
      );
      await tester.runAsync(hh.load);
      await tester.pump();

      // An app with no link offers no Share….
      // The menu's Open item: there once the menu is up.
      final menuOpen = find.byKey(const ValueKey('open'));
      bool menuGone() => menuOpen.evaluate().isEmpty;
      unawaited(homeFlows.menu(other, Rect.zero, const []));
      await settle(tester, menuOpen);
      expect(find.text('Share…'), findsNothing);
      await tester.tapAt(const Offset(10, 10));
      await settleUntil(tester, menuGone);

      unawaited(homeFlows.menu(aid, Rect.zero, const []));
      await settle(tester, find.text('Share…'));
      final publish = tester.getTopLeft(find.textContaining('Publish to'));
      final share = tester.getTopLeft(find.text('Share…'));
      expect(share.dy, greaterThan(publish.dy));
      await tester.tap(find.text('Share…'));
      await settle(tester, find.text('People with access'));
      expect(find.text('Share Plant log'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('share-done')));
      await settleUntil(
        tester,
        () => find.byType(ShareSheet).evaluate().isEmpty,
      );

      // A recipient's device may not share: no Share….
      await tester.runAsync(
        () => me.store.update(
          aid,
          (e) async => e!.copyWith(
            link: () => e.link!.copyWith(role: AppLink.recipient),
          ),
        ),
      );
      unawaited(homeFlows.menu(aid, Rect.zero, const []));
      await settle(tester, menuOpen);
      expect(find.text('Open'), findsOneWidget);
      expect(find.text('Share…'), findsNothing);
      await tester.tapAt(const Offset(10, 10));
      await settleUntil(tester, menuGone);
    });
  });

  test('the clipboard default copies the text', () async {
    TestWidgetsFlutterBinding.ensureInitialized();
    String? got;
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform, (call) async {
          if (call.method == 'Clipboard.setData') {
            got = (call.arguments as Map)['text'] as String?;
          }
          return null;
        });
    await copyToClipboard('https://clax.example/a/x');
    expect(got, 'https://clax.example/a/x');
  });
}
