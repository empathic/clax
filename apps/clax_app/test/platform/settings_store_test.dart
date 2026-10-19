import 'dart:io';

import 'package:clax_app/platform/settings_store.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('developer tools are off by default', () {
    expect(AppSettings().developer, isFalse);
    expect(AppSettings.fromJson(const {}).developer, isFalse);
  });

  test('developer round-trips through toJson and fromJson', () {
    final on = AppSettings(developer: true);
    expect(on.toJson()['developer'], isTrue);
    expect(AppSettings.fromJson(on.toJson()).developer, isTrue);
    expect(on.copy().developer, isTrue);
    expect(AppSettings.fromJson(AppSettings().toJson()).developer, isFalse);
  });

  test('a developer value that is not a bool is ignored', () {
    for (final v in ['true', 1, null, <Object?>[]]) {
      expect(AppSettings.fromJson({'developer': v}).developer, isFalse);
    }
  });

  test('developer is kept in the settings file', () async {
    final dir = Directory.systemTemp.createTempSync('clax-settings-store-');
    addTearDown(() => dir.deleteSync(recursive: true));
    final file = File('${dir.path}/settings.json');
    await SettingsStore(file).save(AppSettings(developer: true));
    expect((await SettingsStore(file).load()).developer, isTrue);
  });

  test('the rallied apps round-trip, and anything but strings is dropped', () {
    expect(AppSettings().rallied, isEmpty);
    final s = AppSettings(rallied: ['a1', 'a2']);
    expect(AppSettings.fromJson(s.toJson()).rallied, ['a1', 'a2']);
    final copy = s.copy()..rallied.add('a3');
    expect(s.rallied, ['a1', 'a2'], reason: 'a copy has a list of its own');
    expect(copy.rallied, ['a1', 'a2', 'a3']);
    expect(
      AppSettings.fromJson({
        'rallied': ['a1', 3, null, 'a2'],
      }).rallied,
      ['a1', 'a2'],
    );
    expect(AppSettings.fromJson({'rallied': 'a1'}).rallied, isEmpty);
  });
}
