import 'dart:convert';
import 'dart:io';

import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/platform/secure_key.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/settings_screen.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:flutter_test/flutter_test.dart';

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
    .descendant(of: find.byType(ListView), matching: find.byType(Scrollable))
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
    test('a missing file loads the defaults', () async {
      final s = await SettingsStore(file).load();
      expect(s.backend, 'messages-api');
      expect(s.model, 'claude-haiku-5-5');
      expect(s.dictateComments, isTrue);
      expect(s.dictationCues, isTrue);
      expect(s.claudePath, isNull);
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

    test(
      "the comment button's place round trips; a bad one is dropped",
      () async {
        final store = SettingsStore(file);
        expect((await store.load()).commentButtonDy, isNull);
        await store.save(AppSettings(commentButtonDy: 212.5));
        expect(
          jsonDecode(file.readAsStringSync()),
          containsPair('comment_button_dy', 212.5),
        );
        expect((await store.load()).commentButtonDy, 212.5);
        expect(store.last.commentButtonDy, 212.5);
        for (final bad in [-3, 'high', null]) {
          file.writeAsStringSync(jsonEncode({'comment_button_dy': bad}));
          expect((await store.load()).commentButtonDy, isNull);
        }
      },
    );

    test('a broken file or unknown values load the defaults', () async {
      file.writeAsStringSync('{not json');
      expect((await SettingsStore(file).load()).model, 'claude-haiku-5-5');
      file.writeAsStringSync(
        jsonEncode({'backend': 'telepathy', 'model': 'gpt', 'dictate': 3}),
      );
      final s = await SettingsStore(file).load();
      expect(s.backend, 'messages-api');
      expect(s.model, 'claude-haiku-5-5');
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
  }) async {
    await tester.pumpWidget(
      MaterialApp(
        home: SettingsScreen(
          store: store,
          secureKey: key,
          cliSupported: cliSupported,
          timings: timings,
        ),
      ),
    );
    await tester.runAsync(() => Future<void>.delayed(Duration.zero));
    await tester.pumpAndSettle();
  }

  testWidgets('the CLI agent option is absent when it is not supported', (
    tester,
  ) async {
    await pumpSettings(tester, store: SettingsStore(file), key: FakeKey());
    expect(find.text('Claude API key'), findsOneWidget);
    expect(find.text('Claude Code on this Mac'), findsNothing);
    expect(find.text('Claude Code path'), findsNothing);
    expect(find.text('Fast (Haiku 5.5)'), findsOneWidget);
    expect(find.text('Balanced (Sonnet 5.5)'), findsOneWidget);
    expect(find.text('Best (Opus 5.5)'), findsOneWidget);
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
    await tester.tap(find.text('Claude Code on this Mac'));
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
    await tester.tap(find.text('Best (Opus 5.5)'));
    await tester.pumpAndSettle();
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
    await tester.tap(find.text('Balanced (Sonnet 5.5)'));
    await tester.runAsync(() => Future<void>.delayed(Duration.zero));
    await tester.pumpAndSettle();
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

  testWidgets('recent timings are listed', (tester) async {
    await pumpSettings(
      tester,
      store: SettingsStore(file),
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
}
