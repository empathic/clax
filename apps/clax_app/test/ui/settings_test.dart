import 'dart:convert';
import 'dart:io';

import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/bundle/installer.dart';
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/home/home_install.dart';
import 'package:clax_app/home/home_recovery.dart';
import 'package:clax_app/home/home_settings.dart';
import 'package:clax_app/platform/secure_key.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/ai_consent_sheet.dart';
import 'package:clax_app/ui/haiku.dart';
import 'package:clax_app/ui/kit/kit.dart';
import 'package:clax_app/ui/settings_screen.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/version.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/paths.dart';

class FakeKey implements SecureKey {
  FakeKey([this.value]);
  String? value;
  final List<String?> writes = [];

  @override
  Future<String?> read() async => value;

  @override
  Future<void> write(String? key) async {
    writes.add(key);
    value = (key == null || key.isEmpty) ? null : key;
  }
}

/// The settings list's own scrollable, not a text field's.
final _list = find
    .descendant(of: find.byType(CustomScrollView), matching: find.byType(Scrollable))
    .first;

void main() {
  late Directory dir;
  late File file;

  setUp(() {
    dir = Directory.systemTemp.createTempSync('clax-settings-');
    file = File('${dir.path}/settings.json');
  });
  tearDown(() => dir.deleteSync(recursive: true));

  group('SettingsStore', () {
    test('the AI consent round trips; a missing or bad value is none', () async {
      final store = SettingsStore(file);
      expect((await store.load()).aiConsent, isNull);
      await store.save(AppSettings()..aiConsent = true);
      expect(
        jsonDecode(file.readAsStringSync()),
        containsPair('ai_consent', true),
      );
      expect((await store.load()).aiConsent, isTrue);
      await store.save(AppSettings()..aiConsent = false);
      expect((await store.load()).aiConsent, isFalse);
      expect(AppSettings(aiConsent: true).copy().aiConsent, isTrue);
      for (final bad in [1, 'yes']) {
        file.writeAsStringSync(jsonEncode({'ai_consent': bad}));
        expect((await store.load()).aiConsent, isNull);
      }
    });

    test('a missing file loads the defaults', () async {
      final s = await SettingsStore(file).load();
      expect(s.backend, 'messages-api');
      expect(s.model, 'claude-sonnet-5-5');
      expect(s.dictateComments, isTrue);
      expect(s.dictationCues, isTrue);
      expect(s.claudePath, isNull);
    });

    test('an install that saved Haiku keeps it', () async {
      file.writeAsStringSync(jsonEncode({'model': 'claude-haiku-5-5'}));
      expect((await SettingsStore(file).load()).model, 'claude-haiku-5-5');
    });

    test('settings round trip through the file', () async {
      final store = SettingsStore(file);
      await store.save(
        AppSettings(
          backend: 'claude-cli',
          model: 'claude-opus-5-5',
          dictateComments: false,
          dictationCues: false,
          claudePath: '/opt/claude',
        ),
      );
      expect(
        jsonDecode(file.readAsStringSync()),
        containsPair('dictation_cues', false),
      );
      final s = await store.load();
      expect(s.dictationCues, isFalse);
      expect(s.backend, 'claude-cli');
      expect(s.model, 'claude-opus-5-5');
      expect(s.dictateComments, isFalse);
      expect(s.claudePath, '/opt/claude');
    });

    test('the first-run hint being seen round trips; a bad value is '
        'unseen', () async {
      final store = SettingsStore(file);
      expect((await store.load()).appHintSeen, isFalse);
      await store.save(AppSettings(appHintSeen: true));
      expect(
        jsonDecode(file.readAsStringSync()),
        containsPair('app_hint_seen', true),
      );
      expect((await store.load()).appHintSeen, isTrue);
      expect(store.last.appHintSeen, isTrue);
      for (final bad in [1, 'yes', null]) {
        file.writeAsStringSync(jsonEncode({'app_hint_seen': bad}));
        expect((await store.load()).appHintSeen, isFalse);
      }
    });

    test('the author name round trips trimmed; empty, long or odd values '
        'are none', () async {
      final store = SettingsStore(file);
      expect((await store.load()).authorName, isNull);
      await store.save(AppSettings(authorName: 'Alex'));
      expect(
        jsonDecode(file.readAsStringSync()),
        containsPair('author_name', 'Alex'),
      );
      expect((await store.load()).authorName, 'Alex');
      expect(store.last.authorName, 'Alex');
      file.writeAsStringSync(jsonEncode({'author_name': '  Sam  '}));
      expect((await store.load()).authorName, 'Sam');
      for (final bad in ['', '   ', 'x' * 65, 3, null]) {
        file.writeAsStringSync(jsonEncode({'author_name': bad}));
        expect((await store.load()).authorName, isNull, reason: '$bad');
      }
    });

    test('a broken file or unknown values load the defaults', () async {
      file.writeAsStringSync('{not json');
      expect((await SettingsStore(file).load()).model, 'claude-sonnet-5-5');
      file.writeAsStringSync(
        jsonEncode({'backend': 'telepathy', 'model': 'gpt', 'dictate': 3}),
      );
      final s = await SettingsStore(file).load();
      expect(s.backend, 'messages-api');
      expect(s.model, 'claude-sonnet-5-5');
      expect(s.dictateComments, isTrue);
      expect(s.dictationCues, isTrue);
    });

    test('last is the defaults, then what was last loaded or saved', () async {
      final store = SettingsStore(file);
      expect(store.last.dictationCues, isTrue);
      file.writeAsStringSync(jsonEncode({'dictation_cues': false}));
      await store.load();
      expect(store.last.dictationCues, isFalse);
      await store.save(AppSettings(dictateComments: false));
      expect(store.last.dictationCues, isTrue);
      expect(store.last.dictateComments, isFalse);
    });
  });

  test('the key store uses the login keychain on macOS', () {
    expect(FlutterSecureKey.macOsOptions.usesDataProtectionKeychain, isFalse);
    expect(FlutterSecureKey.storageKey, 'anthropic_api_key');
    // Built with the defaults it must not throw.
    expect(FlutterSecureKey(), isA<SecureKey>());
    expect(
      const MacOsOptions(usesDataProtectionKeychain: false).toMap(),
      FlutterSecureKey.macOsOptions.toMap(),
    );
  });

  Future<void> pumpSettings(
    WidgetTester tester, {
    required SettingsStore store,
    required SecureKey key,
    bool cliSupported = false,
    List<TimingRecord> timings = const [],
    Future<String?> Function()? fingerprint,
    HomeScreenControls? home,
    Future<String?> Function()? homeVersion,
    List<String> haikus = const ['One haiku', 'Two haiku', 'Three haiku'],
    TargetPlatform platform = TargetPlatform.android,
    bool asksAiConsent = false,
  }) async {
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.light, platform),
        home: SettingsScreen(
          store: store,
          secureKey: key,
          cliSupported: cliSupported,
          timings: timings,
          fingerprint: fingerprint,
          home: home,
          homeVersion: homeVersion,
          haikus: () async => haikus,
          asksAiConsent: asksAiConsent,
        ),
      ),
    );
    await tester.runAsync(() => Future<void>.delayed(Duration.zero));
    await tester.pumpAndSettle();
  }

  Future<void> chooseModel(WidgetTester tester, String word) async {
    await tester.tap(find.text('Model'));
    await tester.pumpAndSettle();
    await tester.tap(find.text(word).last);
    await tester.runAsync(() => Future<void>.delayed(Duration.zero));
    await tester.pumpAndSettle();
  }

  /// A view tall enough that every row of Settings is built at once.
  void tall(WidgetTester tester, [double width = 412]) {
    tester.view.physicalSize = Size(width, 4000);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
  }

  HomeScreenControls homeControls() => HomeScreenControls(
    settings: HomeSettings.loadSync(dir),
    open: () {},
    changed: () {},
  );

  group('Layout', () {
    testWidgets('sections run Claude, Dictation, Sharing, Home screen, '
        'About, Developer', (tester) async {
      tall(tester);
      await pumpSettings(
        tester,
        store: SettingsStore(file),
        key: FakeKey(),
        home: homeControls(),
      );
      final order = [
        'Claude',
        'Dictation',
        'Sharing',
        'Home screen',
        'About',
        'Developer',
      ];
      final tops = [
        for (final h in order) tester.getTopLeft(find.text(h).first).dy,
      ];
      for (var i = 1; i < tops.length; i++) {
        expect(tops[i], greaterThan(tops[i - 1]), reason: order[i]);
      }
      expect(find.text('Agent'), findsNothing);
    });

    testWidgets('every row starts at one 16 dp inset, with no leading icons', (
      tester,
    ) async {
      tall(tester);
      final store = SettingsStore(file);
      await tester.runAsync(() => store.save(AppSettings(developer: true)));
      await pumpSettings(
        tester,
        store: store,
        key: FakeKey('sk-ant-old'),
        home: homeControls(),
        homeVersion: () async => '1.6',
      );
      final tiles = find.byType(ListTile);
      expect(tiles, findsWidgets);
      for (final e in tiles.evaluate()) {
        final tile = e.widget as ListTile;
        expect(tile.leading, isNull);
        final left = tester.getTopLeft(find.byWidget(tile)).dx;
        final title = find.descendant(
          of: find.byWidget(tile),
          matching: find.byType(Text),
        );
        expect(
          tester.getTopLeft(title.first).dx - left,
          16,
          reason: '${(tester.widget<Text>(title.first)).data}',
        );
        expect(left, 0);
      }
      for (final label in [
        'Claude',
        'About',
        'Developer tools',
        'Your name',
      ]) {
        expect(
          tester.getTopLeft(find.text(label).first).dx,
          greaterThanOrEqualTo(16),
          reason: label,
        );
      }
      expect(tester.getTopLeft(find.text('Claude')).dx, 16);
      expect(tester.getTopLeft(find.text('Developer tools')).dx, 16);
      expect(
        tester.getTopLeft(find.byKey(const Key('author-name-field'))).dx,
        16,
      );
    });

    testWidgets('on a wide screen the column is at most 640, centred', (
      tester,
    ) async {
      tall(tester, 1280);
      await pumpSettings(tester, store: SettingsStore(file), key: FakeKey());
      final field = find.byKey(const Key('api-key-field'));
      final r = tester.getRect(field);
      expect(r.left, (1280 - 640) / 2 + 16);
      expect(r.right, (1280 + 640) / 2 - 16);
    });

    testWidgets('the large title collapses, with a line under it once '
        'content is under it', (tester) async {
      await pumpSettings(tester, store: SettingsStore(file), key: FakeKey());
      Color line() =>
          tester
                  .widget<ColoredBox>(
                    find.byKey(const Key('clax-scrolled-under-line')),
                  )
                  .color;
      expect(line().a, 0);
      await tester.drag(find.text('Model'), const Offset(0, -600));
      await tester.pumpAndSettle();
      final p = claxPalette(tester.element(find.byType(SettingsScreen)));
      expect(line(), p.border);
    });
  });

  group('Claude', () {
    testWidgets('where Clax asks first, an allowed consent shows with Turn '
        'off, which takes it back so the next run asks again', (tester) async {
      tall(tester);
      final store = SettingsStore(file);
      await store.save(AppSettings(aiConsent: true));
      await pumpSettings(
        tester,
        store: store,
        key: FakeKey(),
        asksAiConsent: true,
      );
      expect(find.text('Requests to Anthropic'), findsOneWidget);
      expect(find.text(aiConsentAllowed), findsOneWidget);
      await tester.tap(find.text('Turn off'));
      await tester.runAsync(() => Future<void>.delayed(Duration.zero));
      await tester.pumpAndSettle();
      expect((await store.load()).aiConsent, isFalse);
      expect(find.text(aiConsentAsks), findsOneWidget);
      expect(find.text('Turn off'), findsNothing);
    });

    testWidgets('where Clax asks first and nothing is allowed yet, the row '
        'says so, with nothing to turn off', (tester) async {
      tall(tester);
      await pumpSettings(
        tester,
        store: SettingsStore(file),
        key: FakeKey(),
        asksAiConsent: true,
      );
      expect(find.text(aiConsentAsks), findsOneWidget);
      expect(find.text('Turn off'), findsNothing);
    });

    testWidgets('where Clax does not ask (Android, macOS) there is no row', (
      tester,
    ) async {
      tall(tester);
      final store = SettingsStore(file);
      await store.save(AppSettings(aiConsent: true));
      await pumpSettings(tester, store: store, key: FakeKey());
      expect(find.text('Requests to Anthropic'), findsNothing);
    });

    testWidgets('the key field is a ClaxTextField', (tester) async {
      await pumpSettings(tester, store: SettingsStore(file), key: FakeKey());
      expect(
        tester.widget(find.byKey(const Key('api-key-field'))),
        isA<ClaxTextField>(),
      );
      final save = tester.widget<ClaxButton>(
        find.widgetWithText(ClaxButton, 'Save key'),
      );
      expect(save.kind, ClaxButtonKind.tonal);
    });

    testWidgets('a saved key offers Replace, and Remove as a danger action', (
      tester,
    ) async {
      await pumpSettings(
        tester,
        store: SettingsStore(file),
        key: FakeKey('sk-ant-old'),
      );
      final remove = tester.widget<ClaxButton>(
        find.widgetWithText(ClaxButton, 'Remove'),
      );
      expect(remove.kind, ClaxButtonKind.destructive);
      final p = claxPalette(tester.element(find.text('Remove')));
      final style = DefaultTextStyle.of(tester.element(find.text('Remove')));
      expect(style.style.color, p.danger);
      expect(
        tester.widget<ClaxButton>(find.widgetWithText(ClaxButton, 'Replace')).kind,
        ClaxButtonKind.plain,
      );
    });

    testWidgets('Model is one row opening Fast, Balanced and Best, each with '
        'its model', (tester) async {
      final store = SettingsStore(file);
      await pumpSettings(tester, store: store, key: FakeKey());
      expect(find.text('Balanced'), findsOneWidget);
      expect(find.text('Fast'), findsNothing);
      await tester.tap(find.text('Model'));
      await tester.pumpAndSettle();
      for (final (word, name) in [
        ('Fast', 'Haiku 5.5'),
        ('Balanced', 'Sonnet 5.5'),
        ('Best', 'Opus 5.5'),
      ]) {
        final item = find.ancestor(
          of: find.text(word).last,
          matching: find.byType(MenuItemButton),
        );
        expect(
          find.descendant(of: item, matching: find.text(name)),
          findsOneWidget,
          reason: word,
        );
      }
      await tester.tap(find.text('Fast').last);
      await tester.runAsync(() => Future<void>.delayed(Duration.zero));
      await tester.pumpAndSettle();
      expect(find.text('Fast'), findsOneWidget);
      expect(find.byType(MenuItemButton), findsNothing);
      await tester.runAsync(() async {
        expect((await store.load()).model, 'claude-haiku-5-5');
      });
    });

    test('the words name modelChoices fastest first', () {
      expect([for (final m in modelChoices) m.word], [
        'Fast',
        'Balanced',
        'Best',
      ]);
      expect([for (final m in modelChoices) m.name], [
        'Haiku 5.5',
        'Sonnet 5.5',
        'Opus 5.5',
      ]);
    });
  });

  group('About', () {
    Future<void> showAbout(WidgetTester tester) async {
      await tester.scrollUntilVisible(
        find.byType(HaikuLine),
        200,
        scrollable: _list,
      );
      await tester.pumpAndSettle();
    }

    String haiku(WidgetTester tester) =>
        tester.widget<HaikuLine>(find.byType(HaikuLine)).text;

    testWidgets("shows Clax's version and the home screen's", (tester) async {
      await pumpSettings(
        tester,
        store: SettingsStore(file),
        key: FakeKey(),
        homeVersion: () async => '1.6',
      );
      await showAbout(tester);
      expect(find.text('Version $claxVersion'), findsOneWidget);
      expect(find.text('Version 1.6'), findsOneWidget);
    });

    testWidgets('a haiku shows, a new one each visit', (tester) async {
      const list = ['One haiku', 'Two haiku', 'Three haiku'];
      final seen = <String>[];
      for (var visit = 0; visit < 4; visit++) {
        await tester.pumpWidget(const SizedBox());
        await pumpSettings(
          tester,
          store: SettingsStore(file),
          key: FakeKey(),
          haikus: list,
        );
        await showAbout(tester);
        seen.add(haiku(tester));
        expect(list, contains(seen.last));
        if (visit > 0) expect(seen[visit], isNot(seen[visit - 1]));
      }
    });

    testWidgets('a long-press on the haiku shows another', (tester) async {
      await pumpSettings(tester, store: SettingsStore(file), key: FakeKey());
      await showAbout(tester);
      final before = haiku(tester);
      await tester.longPress(find.byType(HaikuLine));
      await tester.pumpAndSettle();
      expect(haiku(tester), isNot(before));
    });

    testWidgets('with no haikus About keeps its versions', (tester) async {
      await pumpSettings(
        tester,
        store: SettingsStore(file),
        key: FakeKey(),
        haikus: const [],
      );
      await tester.scrollUntilVisible(
        find.text('Version $claxVersion'),
        200,
        scrollable: _list,
      );
      expect(find.byType(HaikuLine), findsNothing);
    });

    test("Clax's version is pubspec.yaml's", () {
      final pubspec = File(appPath('pubspec.yaml')).readAsStringSync();
      final m = RegExp(r'^version: ([^+\s]+)', multiLine: true)
          .firstMatch(pubspec)!;
      expect(claxVersion, m.group(1));
    });
  });

  testWidgets('the CLI agent option is absent when it is not supported', (
    tester,
  ) async {
    await pumpSettings(tester, store: SettingsStore(file), key: FakeKey());
    expect(find.text('Claude API key'), findsOneWidget);
    expect(find.text('Claude Code on this Mac'), findsNothing);
    expect(find.text('Claude Code path'), findsNothing);
    expect(find.text('Agent'), findsNothing);
    expect(find.text('Model'), findsOneWidget);
    await tester.scrollUntilVisible(
      find.text('Dictation sounds and vibration'),
      100,
      scrollable: _list,
    );
    expect(find.text('Start dictation automatically'), findsOneWidget);
    expect(find.text('Dictation sounds and vibration'), findsOneWidget);
  });

  testWidgets('with CLI support the agent can be switched and is saved', (
    tester,
  ) async {
    final store = SettingsStore(file);
    await pumpSettings(
      tester,
      store: store,
      key: FakeKey(),
      cliSupported: true,
    );
    expect(find.text('Claude API'), findsOneWidget);
    await tester.tap(find.text('Agent'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Claude Code on this Mac').last);
    await tester.runAsync(() => Future<void>.delayed(Duration.zero));
    await tester.pumpAndSettle();
    expect(find.text('Claude Code path'), findsOneWidget);
    await tester.runAsync(() async {
      expect((await store.load()).backend, 'claude-cli');
    });
  });

  testWidgets('choosing a model and dictation saves them', (tester) async {
    final store = SettingsStore(file);
    await pumpSettings(tester, store: store, key: FakeKey());
    await chooseModel(tester, 'Best');
    for (final label in [
      'Start dictation automatically',
      'Dictation sounds and vibration',
    ]) {
      await tester.scrollUntilVisible(find.text(label), 100, scrollable: _list);
      await tester.tap(find.text(label));
      await tester.runAsync(() => Future<void>.delayed(Duration.zero));
      await tester.pumpAndSettle();
    }
    await tester.runAsync(() async {
      final s = await store.load();
      expect(s.model, 'claude-opus-5-5');
      expect(s.dictateComments, isFalse);
      expect(s.dictationCues, isFalse);
    });
    expect(store.last.dictationCues, isFalse);
  });

  testWidgets('the key is masked and saved through SecureKey, never the file', (
    tester,
  ) async {
    const secret = 'sk-ant-test-0123456789';
    final key = FakeKey();
    final store = SettingsStore(file);
    await pumpSettings(tester, store: store, key: key);
    final field = find.byKey(const Key('api-key-field'));
    await tester.enterText(field, secret);
    await tester.pump();
    final editable = tester.widget<EditableText>(
      find.descendant(of: field, matching: find.byType(EditableText)),
    );
    expect(editable.obscureText, isTrue);
    await tester.tap(find.text('Save key'));
    await tester.runAsync(() => Future<void>.delayed(Duration.zero));
    await tester.pumpAndSettle();
    expect(key.writes, [secret]);
    expect(find.text('A key is saved'), findsOneWidget);
    // Any later settings write still leaves the key out of the file.
    await chooseModel(tester, 'Fast');
    expect(file.existsSync(), isTrue);
    expect(file.readAsStringSync(), isNot(contains(secret)));
  });

  testWidgets('a saved key can be removed', (tester) async {
    final key = FakeKey('sk-ant-old');
    await pumpSettings(tester, store: SettingsStore(file), key: key);
    expect(find.text('A key is saved'), findsOneWidget);
    expect(find.textContaining('sk-ant-old'), findsNothing);
    await tester.tap(find.text('Remove'));
    await tester.runAsync(() => Future<void>.delayed(Duration.zero));
    await tester.pumpAndSettle();
    expect(key.writes, [null]);
    expect(find.text('A key is saved'), findsNothing);
  });

  testWidgets('recent timings are listed with Developer tools on', (
    tester,
  ) async {
    final store = SettingsStore(file);
    await store.save(AppSettings(developer: true));
    await pumpSettings(
      tester,
      store: store,
      key: FakeKey(),
      timings: [
        TimingRecord(
          name: sayUsable,
          elapsed: const Duration(milliseconds: 8400),
          at: DateTime.utc(2026, 10, 12, 9),
        ),
      ],
    );
    await tester.scrollUntilVisible(find.text('8.4 s'), 100, scrollable: _list);
    expect(find.text('Recent timings'), findsOneWidget);
    expect(find.text('Say → usable'), findsOneWidget);
    expect(find.text('8.4 s'), findsOneWidget);
  });

  testWidgets('settings fit a small phone', (tester) async {
    tester.view.physicalSize = const Size(320, 568);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    await pumpSettings(
      tester,
      store: SettingsStore(file),
      key: FakeKey('sk-ant-old'),
      cliSupported: true,
    );
    expect(tester.takeException(), isNull);
    expect(find.text('A key is saved'), findsOneWidget);
  });

  testWidgets('the key field says where a key comes from', (tester) async {
    await pumpSettings(tester, store: SettingsStore(file), key: FakeKey());
    expect(
      find.text('Create one at console.anthropic.com, then paste it here'),
      findsOneWidget,
    );
  });

  testWidgets('Paste fills the key field from the clipboard', (tester) async {
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      (call) async => call.method == 'Clipboard.getData'
          ? {'text': '  sk-ant-pasted-key \n'}
          : null,
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        null,
      ),
    );
    final key = FakeKey();
    await pumpSettings(tester, store: SettingsStore(file), key: key);
    await tester.tap(find.byTooltip('Paste'));
    await tester.pumpAndSettle();
    final field = find.byKey(const Key('api-key-field'));
    expect(
      tester
          .widget<EditableText>(
            find.descendant(of: field, matching: find.byType(EditableText)),
          )
          .controller
          .text,
      'sk-ant-pasted-key',
    );
    await tester.tap(find.text('Save key'));
    await tester.pumpAndSettle();
    expect(key.writes, ['sk-ant-pasted-key']);
  });

  testWidgets('a key without the sk-ant- prefix is not saved', (tester) async {
    final key = FakeKey();
    await pumpSettings(tester, store: SettingsStore(file), key: key);
    await tester.enterText(
      find.byKey(const Key('api-key-field')),
      'hello there',
    );
    await tester.pump();
    await tester.tap(find.text('Save key'));
    await tester.pumpAndSettle();
    expect(key.writes, isEmpty);
    expect(find.text('Claude API keys start with sk-ant-'), findsOneWidget);
  });

  group('Sharing', () {
    testWidgets('Your name is saved as it is typed', (tester) async {
      final store = SettingsStore(file);
      await pumpSettings(tester, store: store, key: FakeKey());
      final field = find.byKey(const Key('author-name-field'));
      await tester.scrollUntilVisible(field, 100, scrollable: _list);
      expect(find.text('Sharing'), findsOneWidget);
      await tester.enterText(field, ' Alex ');
      await tester.runAsync(() => Future<void>.delayed(Duration.zero));
      await tester.pumpAndSettle();
      await tester.runAsync(() async {
        expect((await store.load()).authorName, 'Alex');
      });
      await tester.enterText(field, '');
      await tester.runAsync(() => Future<void>.delayed(Duration.zero));
      await tester.pumpAndSettle();
      await tester.runAsync(() async {
        expect((await store.load()).authorName, isNull);
      });
    });

    testWidgets('a saved name fills the field; the field takes at most 64 '
        'characters', (tester) async {
      final store = SettingsStore(file);
      await tester.runAsync(() => store.save(AppSettings(authorName: 'Sam')));
      await pumpSettings(tester, store: store, key: FakeKey());
      final field = find.byKey(const Key('author-name-field'));
      await tester.scrollUntilVisible(field, 100, scrollable: _list);
      final editable = tester.widget<EditableText>(
        find.descendant(of: field, matching: find.byType(EditableText)),
      );
      expect(editable.controller.text, 'Sam');
      await tester.enterText(field, 'y' * 80);
      await tester.runAsync(() => Future<void>.delayed(Duration.zero));
      await tester.pumpAndSettle();
      await tester.runAsync(() async {
        expect((await store.load()).authorName, 'y' * 64);
      });
    });

    testWidgets('the key fingerprint shows once a key exists', (tester) async {
      await pumpSettings(
        tester,
        store: SettingsStore(file),
        key: FakeKey(),
        fingerprint: () async => '7KQ2-M9XD-4FTA-QW3B',
      );
      await tester.scrollUntilVisible(
        find.text('7KQ2-M9XD-4FTA-QW3B'),
        100,
        scrollable: _list,
      );
      expect(find.text('Your sharing key'), findsOneWidget);
    });

    testWidgets('before the first share there is no fingerprint', (
      tester,
    ) async {
      await pumpSettings(
        tester,
        store: SettingsStore(file),
        key: FakeKey(),
        fingerprint: () async => null,
      );
      await tester.scrollUntilVisible(
        find.text('A sharing key is made the first time you share an app'),
        100,
        scrollable: _list,
      );
      expect(find.text('Your sharing key'), findsNothing);
    });
  });
  testWidgets("About's Licences opens the licences page under Clax's copyright", (
    tester,
  ) async {
    await pumpSettings(tester, store: SettingsStore(file), key: FakeKey());
    await tester.scrollUntilVisible(
      find.byKey(const Key('licences')),
      100,
      scrollable: _list,
    );
    // Mid-screen, clear of the pinned bar and the screen's edge.
    await tester.runAsync(
      () => Scrollable.ensureVisible(
        tester.element(find.byKey(const Key('licences'))),
        alignment: 0.5,
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('About'), findsOneWidget);
    await tester.tap(find.byKey(const Key('licences')));
    await tester.pump();
    await tester.pump(const Duration(seconds: 1));
    expect(find.byType(LicensePage), findsOneWidget);
    expect(
      find.textContaining('Copyright (C) 2026 Empathic, Inc.'),
      findsOneWidget,
    );
  });

  group('Home screen', () {
    late FakeClaxApi api;
    late String aid;
    late HomeSettings home;
    late HomeRecovery recovery;
    var opened = 0;
    var changed = 0;

    setUp(() {
      api = FakeClaxApi();
      aid = api.seedArtifact('Home', {'index.html': '<p>1</p>'});
      api.seedVersion(aid, {'index.html': '<p>2</p>'});
      home = HomeSettings.loadSync(dir)..homeAppId = aid;
      recovery = HomeRecovery(
        api: api,
        installer: Installer(
          api: api,
          store: LineageStore(Directory('${dir.path}/support')),
          key: homeInstallKey(),
        ),
        settings: home,
      );
      opened = 0;
      changed = 0;
    });

    Future<void> pump(WidgetTester tester) => pumpSettings(
      tester,
      store: SettingsStore(file),
      key: FakeKey(),
      home: HomeScreenControls(
        settings: home,
        recovery: recovery,
        open: () => opened++,
        changed: () => changed++,
      ),
    );

    Future<void> show(WidgetTester tester, String text) async {
      await tester.scrollUntilVisible(find.text(text), 100, scrollable: _list);
      // Mid-screen, clear of the pinned bar.
      await tester.runAsync(
        () => Scrollable.ensureVisible(
          tester.element(find.text(text)),
          alignment: 0.5,
        ),
      );
      await tester.pumpAndSettle();
    }

    testWidgets('lists Open, Undo, Reset and the built-in switch', (
      tester,
    ) async {
      await pump(tester);
      await show(tester, 'Use the built-in home screen');
      expect(find.text('Home screen'), findsOneWidget);
      expect(find.text('Open in comment mode'), findsOneWidget);
      expect(find.text('Undo last change'), findsOneWidget);
      expect(find.text('Reset to Clax’s home screen'), findsOneWidget);
    });

    testWidgets('Open opens the home app (in comment mode, by the caller)', (
      tester,
    ) async {
      await pump(tester);
      await show(tester, 'Open in comment mode');
      await tester.tap(find.text('Open in comment mode'));
      expect(opened, 1);
    });

    testWidgets('Undo last change restores the version before the latest', (
      tester,
    ) async {
      await pump(tester);
      await show(tester, 'Undo last change');
      await tester.tap(find.text('Undo last change'));
      // Undo saves where it stepped back to with real file I/O: wait for
      // it outside the fake clock.
      final done = find.text('Undid the last change');
      for (var i = 0; i < 200 && done.evaluate().isEmpty; i++) {
        await tester.runAsync(
          () => Future<void>.delayed(const Duration(milliseconds: 10)),
        );
        await tester.pump();
      }
      final r = api.calls.lastWhere((c) => c.name == 'restoreVersion');
      expect(r.args, {'aid': aid, 'n': 1, 'ifVersion': 2});
      expect(done, findsOneWidget);
      expect(HomeSettings.loadSync(dir).undo, (
        app: aid,
        version: 3,
        restored: 1,
      ));
    });

    testWidgets('Use the built-in home screen is saved and told', (
      tester,
    ) async {
      await pump(tester);
      await show(tester, 'Use the built-in home screen');
      await tester.tap(find.text('Use the built-in home screen'));
      expect(changed, 1);
      // The save is real file I/O: wait for it outside the fake clock.
      for (var i = 0; i < 200 && !HomeSettings.loadSync(dir).builtin; i++) {
        await tester.runAsync(
          () => Future<void>.delayed(const Duration(milliseconds: 10)),
        );
        await tester.pump();
      }
      expect(HomeSettings.loadSync(dir).builtin, isTrue);
    });

    testWidgets('without a home app, only the switch acts', (tester) async {
      home.homeAppId = null;
      await pump(tester);
      await show(tester, 'Open in comment mode');
      await tester.tap(find.text('Open in comment mode'));
      await show(tester, 'Undo last change');
      await tester.tap(find.text('Undo last change'));
      await tester.pump();
      expect(opened, 0);
      expect(api.calls.where((c) => c.name == 'restoreVersion'), isEmpty);
    });
  });
}
