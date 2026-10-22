import 'dart:convert';
import 'dart:io';

/// Which artifact is the home app, kept by the app in one JSON file beside
/// `device-grants.json` (`<support>/home.json`):
/// `{"homeAppId": ID or null, "builtin": bool, "keptRelease": semver or
/// null, "undo": {"app": ID, "version": int, "restored": int} or null}`.
///
/// [homeAppId] is the only artifact the `home` capability answers
/// (`HomeHost.call`); [builtin] keeps the built-in home screen as home
/// until switched back; [keptRelease] is the shipped release the person
/// chose to keep their own home app over; [undo] is where Undo last
/// change has stepped back to. A missing, torn or foreign file
/// reads as the defaults (no home app). A save writes a sibling temporary
/// file and renames it over the old one.
class HomeSettings {
  HomeSettings({
    this.homeAppId,
    this.builtin = false,
    this.keptRelease,
    this.undo,
  }) : _file = null;

  HomeSettings._(
    this._file, {
    this.homeAppId,
    this.builtin = false,
    this.keptRelease,
    this.undo,
  });

  /// The file's name in the settings directory.
  static const fileName = 'home.json';

  final File? _file;

  String? homeAppId;
  bool builtin;
  String? keptRelease;

  /// The last Undo of home app `app`: it made version `version` from
  /// version `restored`'s files. The next Undo steps back from `restored`
  /// while `version` is still the app's latest; any other version landing
  /// ends the chain.
  ({String app, int version, int restored})? undo;

  /// The settings kept in [dir]; the defaults when there are none.
  static Future<HomeSettings> load(Directory dir) async {
    final file = File('${dir.path}/$fileName');
    String? text;
    try {
      text = await file.readAsString();
    } on Object {
      // Missing or unreadable: the defaults.
    }
    return _parse(file, text);
  }

  /// [load], reading the file at once: for the first frame of the home
  /// screen.
  static HomeSettings loadSync(Directory dir) {
    final file = File('${dir.path}/$fileName');
    String? text;
    try {
      text = file.readAsStringSync();
    } on Object {
      // Missing or unreadable: the defaults.
    }
    return _parse(file, text);
  }

  static HomeSettings _parse(File file, String? text) {
    if (text == null) return HomeSettings._(file);
    try {
      final j = jsonDecode(text);
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
          undo: switch (j['undo']) {
            {
              'app': final String app,
              'version': final int version,
              'restored': final int restored,
            } =>
              (app: app, version: version, restored: restored),
            _ => null,
          },
        );
      }
    } on Object {
      // Torn: the defaults.
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
        'undo': switch (undo) {
          final u? => {
            'app': u.app,
            'version': u.version,
            'restored': u.restored,
          },
          null => null,
        },
      }),
    );
    await tmp.rename(file.path);
  }
}
