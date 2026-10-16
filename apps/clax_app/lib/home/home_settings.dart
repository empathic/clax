import 'dart:convert';
import 'dart:io';

/// Which artifact is the home app, kept by the app in one JSON file beside
/// `device-grants.json` (`<support>/home.json`):
/// `{"homeAppId": "<id>" | null, "builtin": bool, "keptRelease": "<semver>" | null}`.
///
/// [homeAppId] is the only artifact the `home` capability answers
/// (`HomeHost.call`); [builtin] keeps the built-in home screen as home
/// until switched back; [keptRelease] is the shipped release the person
/// chose to keep their own home app over. A missing, torn or foreign file
/// reads as the defaults (no home app). A save writes a sibling temporary
/// file and renames it over the old one.
class HomeSettings {
  HomeSettings({this.homeAppId, this.builtin = false, this.keptRelease})
    : _file = null;

  HomeSettings._(
    this._file, {
    this.homeAppId,
    this.builtin = false,
    this.keptRelease,
  });

  /// The file's name in the settings directory.
  static const fileName = 'home.json';

  final File? _file;

  String? homeAppId;
  bool builtin;
  String? keptRelease;

  /// The settings kept in [dir]; the defaults when there are none.
  static Future<HomeSettings> load(Directory dir) async {
    final file = File('${dir.path}/$fileName');
    try {
      final j = jsonDecode(await file.readAsString());
      if (j is Map<String, Object?>) {
        return HomeSettings._(
          file,
          homeAppId: switch (j['homeAppId']) {
            final String s when s.isNotEmpty => s,
            _ => null,
          },
          builtin: j['builtin'] == true,
          keptRelease: switch (j['keptRelease']) {
            final String s => s,
            _ => null,
          },
        );
      }
    } on Object {
      // Missing or unreadable: the defaults.
    }
    return HomeSettings._(file);
  }

  /// Writes the settings to the file they were loaded from; nothing for
  /// settings made in memory.
  Future<void> save() async {
    final file = _file;
    if (file == null) return;
    await file.parent.create(recursive: true);
    final tmp = File('${file.path}.tmp');
    await tmp.writeAsString(
      jsonEncode({
        'homeAppId': homeAppId,
        'builtin': builtin,
        'keptRelease': keptRelease,
      }),
    );
    await tmp.rename(file.path);
  }
}
