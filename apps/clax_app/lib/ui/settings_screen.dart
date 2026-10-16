import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../agent/timings.dart';
import '../home/home_recovery.dart';
import '../home/home_settings.dart';
import '../platform/secure_key.dart';
import '../platform/settings_store.dart';

/// Settings: the Claude API key, the agent (where `claude -p` can run), the
/// model, dictation (whether it starts on its own, and its sounds and
/// vibration), the Claude Code path, sharing (the name bundles carry and
/// the sharing key's fingerprint), the home screen ([home]), and the last
/// ten measured timings.
///
/// Every choice is saved to [store] as it is made. The key is saved only
/// through [secureKey], on Save key, and is never shown back: a saved key
/// reads "A key is saved", with Replace and Remove.
class SettingsScreen extends StatefulWidget {
  const SettingsScreen({
    super.key,
    required this.store,
    required this.secureKey,
    required this.cliSupported,
    required this.timings,
    this.fingerprint,
    this.home,
  });

  final SettingsStore store;
  final SecureKey secureKey;

  /// Whether the Claude Code agent can run here (`ClaudeCliBackend.supported`).
  /// Without it the agent choice and the Claude Code path are not offered.
  final bool cliSupported;

  /// Newest first; the first ten are shown.
  final List<TimingRecord> timings;

  /// The sharing key's fingerprint (`BundleIdentity.fingerprint`), or null
  /// before the first export made the key. Read once; never makes a key.
  final Future<String?> Function()? fingerprint;

  /// The home screen's section; null shows none.
  final HomeScreenControls? home;

  @override
  State<SettingsScreen> createState() => _SettingsScreenState();
}

class _SettingsScreenState extends State<SettingsScreen> {
  AppSettings? _s;
  bool _hasKey = false;
  bool _editingKey = false;
  bool _showKey = false;
  final _key = TextEditingController();
  late final TextEditingController _path = TextEditingController();
  final _name = TextEditingController();
  String? _fingerprint;

  @override
  void initState() {
    super.initState();
    _init();
  }

  Future<void> _init() async {
    final s = await widget.store.load();
    String? saved;
    try {
      saved = await widget.secureKey.read();
    } catch (_) {
      saved = null;
    }
    String? fingerprint;
    try {
      fingerprint = await widget.fingerprint?.call();
    } catch (_) {
      fingerprint = null;
    }
    if (!mounted) return;
    setState(() {
      _s = s;
      _hasKey = saved != null;
      _path.text = s.claudePath ?? '';
      _name.text = s.authorName ?? '';
      _fingerprint = fingerprint;
    });
  }

  @override
  void dispose() {
    _key.dispose();
    _path.dispose();
    _name.dispose();
    super.dispose();
  }

  Future<void> _change(void Function(AppSettings s) f) async {
    final s = _s!.copy();
    f(s);
    setState(() => _s = s);
    await widget.store.save(s);
  }

  /// Why the typed key is refused, shown under the field.
  String? _keyError;

  /// The prefix every Claude API key starts with.
  static const keyPrefix = 'sk-ant-';

  Future<void> _saveKey() async {
    final v = _key.text.trim();
    if (v.isEmpty) return;
    if (!v.startsWith(keyPrefix)) {
      setState(() => _keyError = 'Claude API keys start with $keyPrefix');
      return;
    }
    await _writeKey(v);
  }

  Future<void> _paste() async {
    final data = await Clipboard.getData(Clipboard.kTextPlain);
    final text = data?.text?.trim();
    if (text == null || text.isEmpty || !mounted) return;
    _key.value = TextEditingValue(
      text: text,
      selection: TextSelection.collapsed(offset: text.length),
    );
    setState(() => _keyError = null);
  }

  Future<void> _writeKey(String? v) async {
    final messenger = ScaffoldMessenger.of(context);
    try {
      await widget.secureKey.write(v);
    } catch (_) {
      messenger.showSnackBar(
        const SnackBar(content: Text("Couldn't save the key")),
      );
      return;
    }
    if (!mounted) return;
    _key.clear();
    setState(() {
      _hasKey = v != null;
      _editingKey = false;
      _showKey = false;
    });
  }

  @override
  Widget build(BuildContext context) {
    final s = _s;
    return Scaffold(
      appBar: AppBar(title: const Text('Settings')),
      body: s == null
          ? const SizedBox.shrink()
          : ListView(
              padding: const EdgeInsets.only(bottom: 32),
              children: [
                const _Header('Agent'),
                _keyTile(context),
                if (widget.cliSupported) ...[
                  RadioGroup<String>(
                    groupValue: s.backend == backendClaudeCli
                        ? backendClaudeCli
                        : backendMessagesApi,
                    onChanged: (v) {
                      if (v != null) _change((s) => s.backend = v);
                    },
                    child: const Column(
                      children: [
                        RadioListTile(
                          value: backendMessagesApi,
                          title: Text('Claude API'),
                          subtitle: Text('Uses your API key'),
                        ),
                        RadioListTile(
                          value: backendClaudeCli,
                          title: Text('Claude Code on this Mac'),
                          subtitle: Text('Uses your Claude Code login'),
                        ),
                      ],
                    ),
                  ),
                  if (s.backend == backendClaudeCli)
                    Padding(
                      padding: const EdgeInsets.fromLTRB(16, 8, 16, 8),
                      child: TextField(
                        controller: _path,
                        autocorrect: false,
                        decoration: const InputDecoration(
                          labelText: 'Claude Code path',
                          hintText: 'Found on PATH when empty',
                        ),
                        onChanged: (v) => _change(
                          (s) =>
                              s.claudePath = v.trim().isEmpty ? null : v.trim(),
                        ),
                      ),
                    ),
                ],
                const _Header('Model'),
                RadioGroup<String>(
                  groupValue: s.model,
                  onChanged: (v) {
                    if (v != null) _change((s) => s.model = v);
                  },
                  child: Column(
                    children: [
                      for (final m in modelChoices)
                        RadioListTile(value: m.id, title: Text(m.label)),
                    ],
                  ),
                ),
                const _Header('Dictation'),
                SwitchListTile(
                  title: const Text('Start dictation automatically'),
                  subtitle: const Text(
                    'Listen as soon as you start a new app or a comment',
                  ),
                  value: s.dictateComments,
                  onChanged: (v) => _change((s) => s.dictateComments = v),
                ),
                SwitchListTile(
                  title: const Text('Dictation sounds and vibration'),
                  subtitle: const Text(
                    'A tone and a buzz as listening starts and stops',
                  ),
                  value: s.dictationCues,
                  onChanged: (v) => _change((s) => s.dictationCues = v),
                ),
                const _Header('Sharing'),
                Padding(
                  padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
                  child: TextField(
                    key: const Key('author-name-field'),
                    controller: _name,
                    maxLength: maxAuthorName,
                    textCapitalization: TextCapitalization.words,
                    decoration: const InputDecoration(
                      labelText: 'Your name',
                      helperText: 'Shown to people you share apps with',
                      counterText: '',
                    ),
                    onChanged: (v) =>
                        _change((s) => s.authorName = cleanAuthorName(v)),
                  ),
                ),
                if (_fingerprint case final f?)
                  ListTile(
                    leading: const Icon(Icons.fingerprint_rounded),
                    title: const Text('Your sharing key'),
                    subtitle: SelectableText(
                      f,
                      style: const TextStyle(
                        fontFeatures: [FontFeature.tabularFigures()],
                      ),
                    ),
                  )
                else
                  ListTile(
                    leading: const Icon(Icons.fingerprint_rounded),
                    subtitle: const Text(
                      'A sharing key is made the first time you share an app',
                    ),
                  ),
                if (widget.home case final h?) ...[
                  const _Header('Home screen'),
                  _HomeSection(controls: h),
                ],
                const _Header('Recent timings'),
                ..._timings(context),
              ],
            ),
    );
  }

  Widget _keyTile(BuildContext context) {
    if (_hasKey && !_editingKey) {
      return ListTile(
        leading: const Icon(Icons.key_rounded),
        title: const Text('Claude API key'),
        subtitle: const Text('A key is saved'),
        trailing: Wrap(
          spacing: 4,
          children: [
            TextButton(
              onPressed: () => setState(() => _editingKey = true),
              child: const Text('Replace'),
            ),
            TextButton(
              onPressed: () => _writeKey(null),
              child: const Text('Remove'),
            ),
          ],
        ),
      );
    }
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 4, 16, 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          TextField(
            key: const Key('api-key-field'),
            controller: _key,
            obscureText: !_showKey,
            autocorrect: false,
            enableSuggestions: false,
            keyboardType: TextInputType.visiblePassword,
            decoration: InputDecoration(
              labelText: 'Claude API key',
              helperText:
                  'Create one at console.anthropic.com, then paste it here',
              helperMaxLines: 2,
              errorText: _keyError,
              suffixIcon: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  IconButton(
                    tooltip: 'Paste',
                    onPressed: _paste,
                    icon: const Icon(Icons.content_paste_rounded),
                  ),
                  IconButton(
                    tooltip: _showKey ? 'Hide' : 'Show',
                    onPressed: () => setState(() => _showKey = !_showKey),
                    icon: Icon(
                      _showKey
                          ? Icons.visibility_off_outlined
                          : Icons.visibility_outlined,
                    ),
                  ),
                ],
              ),
            ),
            onChanged: (_) => setState(() => _keyError = null),
            onSubmitted: (_) => _saveKey(),
          ),
          const SizedBox(height: 8),
          Row(
            mainAxisAlignment: MainAxisAlignment.end,
            children: [
              if (_editingKey)
                TextButton(
                  onPressed: () => setState(() {
                    _editingKey = false;
                    _key.clear();
                  }),
                  child: const Text('Keep the saved key'),
                ),
              const SizedBox(width: 8),
              FilledButton.tonal(
                onPressed: _key.text.trim().isEmpty ? null : _saveKey,
                child: const Text('Save key'),
              ),
            ],
          ),
        ],
      ),
    );
  }

  List<Widget> _timings(BuildContext context) {
    final recent = widget.timings.take(10).toList();
    final muted = TextStyle(
      color: Theme.of(context).colorScheme.onSurfaceVariant,
    );
    if (recent.isEmpty) {
      return [ListTile(title: Text('Nothing measured yet', style: muted))];
    }
    return [
      for (final t in recent)
        ListTile(
          dense: true,
          title: Text(timingLabel(t.name)),
          subtitle: Text(_when(t.at.toLocal()), style: muted),
          trailing: Text(
            seconds(t.elapsed),
            style: const TextStyle(
              fontFeatures: [FontFeature.tabularFigures()],
            ),
          ),
        ),
    ];
  }

  static String _when(DateTime t) {
    String two(int n) => n.toString().padLeft(2, '0');
    return '${t.year}-${two(t.month)}-${two(t.day)} ${two(t.hour)}:${two(t.minute)}';
  }
}

/// What Settings › Home screen works on: the home app's [settings], the
/// [recovery] Undo and Reset run on, [open] (the home app in comment
/// mode), and [changed], told as [HomeSettings.builtin] changes (before
/// it is saved).
class HomeScreenControls {
  const HomeScreenControls({
    required this.settings,
    required this.open,
    required this.changed,
    this.recovery,
  });

  final HomeSettings settings;
  final HomeRecovery? recovery;
  final VoidCallback open;
  final VoidCallback changed;
}

/// Open, Undo last change, Reset to Clax's home screen (each needs a home
/// app; Undo and Reset a [HomeRecovery] too), and Use the built-in home
/// screen, saved as [HomeSettings.builtin].
class _HomeSection extends StatefulWidget {
  const _HomeSection({required this.controls});
  final HomeScreenControls controls;

  @override
  State<_HomeSection> createState() => _HomeSectionState();
}

class _HomeSectionState extends State<_HomeSection> {
  var _busy = false;

  HomeScreenControls get _c => widget.controls;

  Future<void> _run(
    Future<void> Function(HomeRecovery r) f,
    String done,
  ) async {
    final r = _c.recovery;
    if (r == null) return;
    final messenger = ScaffoldMessenger.of(context);
    setState(() => _busy = true);
    String text;
    try {
      await f(r);
      text = done;
    } on Object catch (e) {
      text = recoveryFailure(e);
    }
    if (!mounted) return;
    setState(() => _busy = false);
    messenger.showSnackBar(SnackBar(content: Text(text)));
  }

  Future<void> _builtin(bool on) async {
    setState(() => _c.settings.builtin = on);
    _c.changed();
    await _c.settings.save();
  }

  @override
  Widget build(BuildContext context) {
    final has = _c.settings.homeAppId != null;
    final can = has && _c.recovery != null && !_busy;
    return Column(
      children: [
        ListTile(
          leading: const Icon(Icons.home_outlined),
          title: const Text('Open'),
          subtitle: const Text('Comment on your home screen to change it'),
          enabled: has,
          onTap: _c.open,
        ),
        ListTile(
          leading: const Icon(Icons.undo_rounded),
          title: const Text('Undo last change'),
          subtitle: const Text('Go back to the version before the latest'),
          enabled: can,
          onTap: () => _run((r) => r.undoLast(), 'Undid the last change'),
        ),
        ListTile(
          leading: const Icon(Icons.restart_alt_rounded),
          title: const Text("Reset to Clax's home screen"),
          subtitle: const Text('Your data and comments stay'),
          enabled: can,
          onTap: () => _run((r) => r.reset(), "Reset to Clax's home screen"),
        ),
        SwitchListTile(
          secondary: const Icon(Icons.grid_view_rounded),
          title: const Text('Use the built-in home screen'),
          subtitle: const Text('Until you switch this off'),
          value: _c.settings.builtin,
          onChanged: _builtin,
        ),
      ],
    );
  }
}

/// A timing's name as Settings shows it.
String timingLabel(String name) => switch (name) {
  sayUsable => 'Say → usable',
  longpressLanded => 'Long-press → landed',
  homeUsable => 'Launch → home usable',
  homeCover => 'Launch → home snapshot',
  _ => name,
};

/// [d] in seconds with one decimal, as `8.4 s`.
String seconds(Duration d) =>
    '${(d.inMilliseconds / 1000).toStringAsFixed(1)} s';

class _Header extends StatelessWidget {
  const _Header(this.text);
  final String text;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 24, 16, 8),
      child: Text(
        text,
        style: theme.textTheme.labelLarge?.copyWith(
          color: theme.colorScheme.primary,
        ),
      ),
    );
  }
}
