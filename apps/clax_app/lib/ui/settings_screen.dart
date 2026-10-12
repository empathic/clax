import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../agent/timings.dart';
import '../platform/secure_key.dart';
import '../platform/settings_store.dart';

/// Settings: the Claude API key, the agent (where `claude -p` can run), the
/// model, dictation (whether it starts on its own, and its sounds and
/// vibration), the Claude Code path, and the last ten measured timings.
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
  });

  final SettingsStore store;
  final SecureKey secureKey;

  /// Whether the Claude Code agent can run here (`ClaudeCliBackend.supported`).
  /// Without it the agent choice and the Claude Code path are not offered.
  final bool cliSupported;

  /// Newest first; the first ten are shown.
  final List<TimingRecord> timings;

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
    if (!mounted) return;
    setState(() {
      _s = s;
      _hasKey = saved != null;
      _path.text = s.claudePath ?? '';
    });
  }

  @override
  void dispose() {
    _key.dispose();
    _path.dispose();
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

/// A timing's name as Settings shows it.
String timingLabel(String name) => switch (name) {
  sayUsable => 'Say → usable',
  longpressLanded => 'Long-press → landed',
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
