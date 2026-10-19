import 'dart:convert';
import 'dart:io';

/// A model the person can choose: its [id], the [word] Settings names it
/// by (Fast, Balanced or Best) and the model's [name].
class ModelChoice {
  const ModelChoice(this.id, this.word, this.name);
  final String id;
  final String word;
  final String name;

  /// [word] with [name], as "Fast (Haiku 5.5)".
  String get label => '$word ($name)';
}

/// The models Settings offers, fastest first.
const modelChoices = [
  ModelChoice('claude-haiku-5-5', 'Fast', 'Haiku 5.5'),
  ModelChoice('claude-sonnet-5-5', 'Balanced', 'Sonnet 5.5'),
  ModelChoice('claude-opus-5-5', 'Best', 'Opus 5.5'),
];

/// The model a new install uses until the person picks another. A saved
/// choice is kept.
const defaultModel = 'claude-sonnet-5-5';

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
    this.dictationCues = true,
    this.claudePath,
    this.appHintSeen = false,
    String? authorName,
    this.developer = false,
    this.aiConsent,
    List<String>? rallied,
  }) : model = model ?? defaultModel,
       authorName = cleanAuthorName(authorName),
       rallied = rallied ?? [];

  /// [backendMessagesApi] or [backendClaudeCli]. The CLI is used only where
  /// `ClaudeCliBackend.supported` holds; elsewhere this setting is ignored.
  String backend;

  /// One of [modelChoices]' IDs.
  String model;

  /// Whether dictation starts on its own: in a new comment's draft and in
  /// an empty New sheet. Shown as "Start dictation automatically".
  bool dictateComments;

  /// Whether dictation vibrates and plays a short tone as it starts and
  /// stops (`VoiceCues`).
  bool dictationCues;

  /// The `claude` executable to run; null finds it on `PATH`.
  String? claudePath;

  /// Whether an app screen showed the first-run hint (where Comment and
  /// Threads are); it shows once per install.
  bool appHintSeen;

  /// The name an exported bundle carries as its author's (`author.name`),
  /// trimmed, 1 to [maxAuthorName] characters; null for none.
  String? authorName;

  /// Whether Clax shows its developer views (`DeveloperScope`): recent
  /// timings, raw JSON, MIME types, capability names and the planned
  /// permission kinds. Shown as "Developer tools"; off by default.
  bool developer;

  /// The person's answer to whether Clax may send their requests to the
  /// Claude API (`AiConsent`): true once allowed, false once taken back in
  /// Settings, null before they were asked. Only where `AiConsent` is
  /// required (iOS) does a run wait for it.
  bool? aiConsent;

  /// The IDs of the apps whose version 10 played Rally of 10, which shows
  /// once per app.
  List<String> rallied;

  AppSettings copy() => AppSettings(
    backend: backend,
    model: model,
    dictateComments: dictateComments,
    dictationCues: dictationCues,
    claudePath: claudePath,
    appHintSeen: appHintSeen,
    authorName: authorName,
    developer: developer,
    aiConsent: aiConsent,
    rallied: List.of(rallied),
  );

  Map<String, Object?> toJson() => {
    'backend': backend,
    'model': model,
    'dictate_comments': dictateComments,
    'dictation_cues': dictationCues,
    'claude_path': claudePath,
    'app_hint_seen': appHintSeen,
    'author_name': authorName,
    'developer': developer,
    'ai_consent': aiConsent,
    'rallied': rallied,
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
    final cues = j['dictation_cues'];
    if (cues is bool) s.dictationCues = cues;
    final path = j['claude_path'];
    if (path is String && path.trim().isNotEmpty) s.claudePath = path.trim();
    final hint = j['app_hint_seen'];
    if (hint is bool) s.appHintSeen = hint;
    final name = j['author_name'];
    if (name is String && name.trim().runes.length <= maxAuthorName) {
      s.authorName = cleanAuthorName(name);
    }
    final developer = j['developer'];
    if (developer is bool) s.developer = developer;
    final consent = j['ai_consent'];
    if (consent is bool) s.aiConsent = consent;
    final rallied = j['rallied'];
    if (rallied is List) s.rallied = rallied.whereType<String>().toList();
    return s;
  }
}

/// At most this many characters in [AppSettings.authorName], as a
/// manifest's `author.name` allows.
const maxAuthorName = 64;

/// [name] trimmed; null when that is empty or longer than [maxAuthorName]
/// characters.
String? cleanAuthorName(String? name) {
  final t = name?.trim();
  if (t == null || t.isEmpty || t.runes.length > maxAuthorName) return null;
  return t;
}

/// Loads and saves [AppSettings] as JSON in one file.
///
/// The file is a few hundred bytes, so it is read and written synchronously
/// inside the async calls; a save writes a sibling temporary file and
/// renames it over the old one, so a crash never leaves half a file.
class SettingsStore {
  SettingsStore(this.file);

  final File file;

  AppSettings _last = AppSettings();

  /// A copy of the settings this store last loaded or saved; the defaults
  /// before the first. For readers that must answer at once, such as a
  /// dictation cue.
  AppSettings get last => _last.copy();

  /// The saved settings; the defaults when the file is missing or broken.
  Future<AppSettings> load() async {
    final s = _read();
    _last = s.copy();
    return s;
  }

  AppSettings _read() {
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
    _last = s.copy();
  }
}
