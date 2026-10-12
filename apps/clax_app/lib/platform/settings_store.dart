import 'dart:convert';
import 'dart:io';

/// A model the person can choose, with the label Settings shows.
class ModelChoice {
  const ModelChoice(this.id, this.label);
  final String id;
  final String label;
}

/// The models Settings offers; the first is the default.
const modelChoices = [
  ModelChoice('claude-haiku-5-5', 'Fast (Haiku 5.5)'),
  ModelChoice('claude-sonnet-5-5', 'Balanced (Sonnet 5.5)'),
  ModelChoice('claude-opus-5-5', 'Best (Opus 5.5)'),
];

/// The Messages API backend's name, as `AgentBackend.name` gives it.
const backendMessagesApi = 'messages-api';

/// The `claude -p` backend's name, as `AgentBackend.name` gives it.
const backendClaudeCli = 'claude-cli';

/// The person's choices, kept in `<support>/settings.json`. The API key is
/// never one of them: it lives in the platform's secure storage
/// (`SecureKey`).
class AppSettings {
  AppSettings({
    this.backend = backendMessagesApi,
    String? model,
    this.dictateComments = true,
    this.claudePath,
  }) : model = model ?? modelChoices.first.id;

  /// [backendMessagesApi] or [backendClaudeCli]. The CLI is used only where
  /// `ClaudeCliBackend.supported` holds; elsewhere this setting is ignored.
  String backend;

  /// One of [modelChoices]' IDs.
  String model;

  /// Whether a new comment's draft starts dictation at once.
  bool dictateComments;

  /// The `claude` executable to run; null finds it on `PATH`.
  String? claudePath;

  AppSettings copy() => AppSettings(
    backend: backend,
    model: model,
    dictateComments: dictateComments,
    claudePath: claudePath,
  );

  Map<String, Object?> toJson() => {
    'backend': backend,
    'model': model,
    'dictate_comments': dictateComments,
    'claude_path': claudePath,
  };

  /// Reads [j], keeping the default for any value that is missing, of the
  /// wrong type, or not one the app knows.
  factory AppSettings.fromJson(Map<String, Object?> j) {
    final s = AppSettings();
    final backend = j['backend'];
    if (backend == backendMessagesApi || backend == backendClaudeCli) {
      s.backend = backend as String;
    }
    final model = j['model'];
    if (modelChoices.any((m) => m.id == model)) s.model = model as String;
    final dictate = j['dictate_comments'];
    if (dictate is bool) s.dictateComments = dictate;
    final path = j['claude_path'];
    if (path is String && path.trim().isNotEmpty) s.claudePath = path.trim();
    return s;
  }
}

/// Loads and saves [AppSettings] as JSON in one file.
///
/// The file is a few hundred bytes, so it is read and written synchronously
/// inside the async calls; a save writes a sibling temporary file and
/// renames it over the old one, so a crash never leaves half a file.
class SettingsStore {
  SettingsStore(this.file);

  final File file;

  /// The saved settings; the defaults when the file is missing or broken.
  Future<AppSettings> load() async {
    try {
      final j = jsonDecode(file.readAsStringSync());
      if (j is Map) return AppSettings.fromJson(j.cast<String, Object?>());
    } on FileSystemException {
      // Never saved yet.
    } on FormatException {
      // Broken: start again from the defaults.
    }
    return AppSettings();
  }

  Future<void> save(AppSettings s) async {
    file.parent.createSync(recursive: true);
    final tmp = File('${file.path}.tmp');
    tmp.writeAsStringSync(
      const JsonEncoder.withIndent('  ').convert(s.toJson()),
      flush: true,
    );
    tmp.renameSync(file.path);
  }
}
