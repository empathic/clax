import 'dart:async';
import 'dart:math' show Random;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../agent/timings.dart';
import '../home/home_recovery.dart';
import '../home/home_settings.dart';
import '../platform/secure_key.dart';
import '../platform/settings_store.dart';
import '../version.dart';
import 'ai_consent_sheet.dart';
import 'developer.dart';
import 'haiku.dart';
import 'kit/kit.dart';
import 'licences.dart';
import 'theme.dart';
import 'tokens.g.dart';

/// Settings, one column of rows at one 16 dp inset under a large title
/// that collapses as it scrolls, at most `form` wide and centred from
/// medium width. Its sections, in order:
/// - Claude: the API key, the agent (where `claude -p` can run, with the
///   Claude Code path), the model, a menu of Fast, Balanced and Best, and,
///   where runs ask first ([asksAiConsent]), whether requests may go to
///   Anthropic, with Turn off once they may;
/// - Dictation: whether it starts on its own, and its sounds and vibration;
/// - Sharing: the name bundles carry and the sharing key's fingerprint;
/// - Notifications, where there are notices ([notificationsAllowed],
///   Android): whether they are on, read again each time Clax comes back
///   to the front; Open settings, the app's page in the system settings
///   (`openAppSettings` on `dev.empathic.clax/native`); and "Show the
///   Comment tab" ([AppSettings.showCommentTab]), which shows it while
///   notifications are on too;
/// - Home screen ([home]);
/// - About: Clax's [version], the home screen's ([homeVersion]), the
///   licences page ([showClaxLicences]) and a haiku, a different one each
///   visit; a long-press (Option-click on the Mac) shows another;
/// - Developer: the "Developer tools" switch ([AppSettings.developer],
///   which also sets the [DeveloperScope] above), with the last ten
///   measured timings while it is on.
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
    this.version = claxVersion,
    this.homeVersion,
    this.haikus,
    this.random,
    this.asksAiConsent = false,
    this.notificationsAllowed,
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

  /// Clax's version, as About shows it.
  final String version;

  /// The home screen's version (its release), or null to show none.
  final Future<String?> Function()? homeVersion;

  /// The haikus About picks from; null reads the app's list
  /// ([loadHaikus]).
  final Future<List<String>> Function()? haikus;

  /// How About picks its haiku; null is a fresh [Random].
  final Random? random;

  /// Whether runs ask before anything goes to the Claude API (`AiConsent`,
  /// iOS): Settings then shows [AppSettings.aiConsent], and Turn off sets it
  /// false, so the next run asks again.
  final bool asksAiConsent;

  /// Whether notifications are on for Clax (`AppNotice.allowed`); null
  /// where Clax shows no notice (macOS), and then there is no
  /// Notifications section.
  final Future<bool> Function()? notificationsAllowed;

  /// The channel Open settings goes through.
  static const native = MethodChannel('dev.empathic.clax/native');

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
  String? _homeVersion;
  List<String> _haikus = const [];
  String? _haiku;

  /// The haiku About showed last, which the next visit does not repeat.
  static String? _lastHaiku;

  /// Whether notifications are on; null before the first read.
  bool? _notifications;
  AppLifecycleListener? _lifecycle;

  @override
  void initState() {
    super.initState();
    _init();
    _loadHaikus();
    if (widget.notificationsAllowed != null) {
      unawaited(_readNotifications());
      // Back from the system settings, they may have changed.
      _lifecycle = AppLifecycleListener(
        onResume: () => unawaited(_readNotifications()),
      );
    }
  }

  Future<void> _readNotifications() async {
    bool on;
    try {
      on = await widget.notificationsAllowed!();
    } on Object {
      on = false;
    }
    if (mounted) setState(() => _notifications = on);
  }

  Future<void> _openAppSettings() async {
    try {
      await SettingsScreen.native.invokeMethod<bool>('openAppSettings');
    } on PlatformException {
      // Nothing to open.
    } on MissingPluginException {
      // Nothing to open.
    }
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
    String? homeVersion;
    try {
      homeVersion = await widget.homeVersion?.call();
    } catch (_) {
      homeVersion = null;
    }
    if (!mounted) return;
    setState(() {
      _s = s;
      _hasKey = saved != null;
      _path.text = s.claudePath ?? '';
      _name.text = s.authorName ?? '';
      _fingerprint = fingerprint;
      _homeVersion = homeVersion;
    });
  }

  /// Reads the haikus apart from the settings, so a slow list never holds
  /// Settings back; About shows its haiku once they are in.
  Future<void> _loadHaikus() async {
    // After initState, where the asset bundle can be looked up.
    await Future<void>.value();
    if (!mounted) return;
    final bundle = DefaultAssetBundle.of(context);
    final List<String> haikus;
    try {
      haikus = await (widget.haikus?.call() ?? loadHaikus(bundle));
    } on Object {
      return;
    }
    if (!mounted) return;
    setState(() {
      _haikus = haikus;
      _haiku = _pick(_lastHaiku);
      _lastHaiku = _haiku;
    });
  }

  /// A haiku other than [not] where there is another; null with none.
  String? _pick(String? not) {
    if (_haikus.isEmpty) return null;
    final others = [for (final h in _haikus) if (h != not) h];
    return pickHaiku(
      others.isEmpty ? _haikus : others,
      random: widget.random,
    );
  }

  void _anotherHaiku() => setState(() => _lastHaiku = _haiku = _pick(_haiku));

  @override
  void dispose() {
    _lifecycle?.dispose();
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
        const SnackBar(content: Text('Couldn’t save the key')),
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
    return ClaxScaffold(
      title: 'Settings',
      largeTitle: true,
      contentWidth: ClaxLayout.form,
      slivers: [
        if (s != null)
          SliverPadding(
            padding: const EdgeInsets.only(bottom: ClaxSpace.s32),
            sliver: SliverList.list(children: _rows(context, s)),
          ),
      ],
    );
  }

  List<Widget> _rows(BuildContext context, AppSettings s) {
    final model = modelChoices.firstWhere(
      (m) => m.id == s.model,
      orElse: () => modelChoices.firstWhere((m) => m.id == defaultModel),
    );
    final cli = s.backend == backendClaudeCli;
    return [
      const _Header('Claude'),
      _keyTile(context),
      if (widget.cliSupported) ...[
        _MenuRow(
          title: 'Agent',
          value: cli ? 'Claude Code on this Mac' : 'Claude API',
          items: [
            ClaxMenuItem(
              backendMessagesApi,
              'Claude API',
              detail: 'Your API key',
              checked: !cli,
            ),
            ClaxMenuItem(
              backendClaudeCli,
              'Claude Code on this Mac',
              detail: 'Your Claude Code login',
              checked: cli,
            ),
          ],
          onChosen: (v) => _change((s) => s.backend = v),
        ),
        if (cli)
          _Inset(
            child: ClaxTextField(
              controller: _path,
              label: 'Claude Code path',
              hint: 'Found on PATH when empty',
              autocorrect: false,
              onChanged: (v) => _change(
                (s) => s.claudePath = v.trim().isEmpty ? null : v.trim(),
              ),
            ),
          ),
      ],
      _MenuRow(
        title: 'Model',
        value: model.word,
        items: [
          for (final m in modelChoices)
            ClaxMenuItem(m.id, m.word, detail: m.name, checked: m == model),
        ],
        onChosen: (v) => _change((s) => s.model = v),
      ),
      if (widget.asksAiConsent) _aiConsentTile(s),
      const _Header('Dictation'),
      _SwitchRow(
        title: 'Start dictation automatically',
        subtitle: 'Listen as soon as you start a new app or a comment',
        value: s.dictateComments,
        onChanged: (v) => _change((s) => s.dictateComments = v),
      ),
      _SwitchRow(
        title: 'Dictation sounds and vibration',
        subtitle: 'A tone and a buzz as listening starts and stops',
        value: s.dictationCues,
        onChanged: (v) => _change((s) => s.dictationCues = v),
      ),
      const _Header('Sharing'),
      _Inset(
        child: ClaxTextField(
          key: const Key('author-name-field'),
          controller: _name,
          label: 'Your name',
          helper: 'Shown to people you share apps with',
          maxLength: maxAuthorName,
          textCapitalization: TextCapitalization.words,
          onChanged: (v) => _change((s) => s.authorName = cleanAuthorName(v)),
        ),
      ),
      if (_fingerprint case final f?)
        ListTile(
          title: const Text('Your sharing key'),
          subtitle: Row(
            children: [
              const Icon(Icons.fingerprint_rounded, size: 18),
              const SizedBox(width: ClaxSpace.s8),
              Flexible(
                child: SelectableText(
                  f,
                  style: const TextStyle(
                    fontFeatures: [FontFeature.tabularFigures()],
                  ),
                ),
              ),
            ],
          ),
        )
      else
        const ListTile(
          subtitle: Text(
            'A sharing key is made the first time you share an app',
          ),
        ),
      if (widget.notificationsAllowed != null) ...[
        const _Header('Notifications'),
        ListTile(
          title: const Text('Notifications for Clax'),
          subtitle: Text(
            _notifications == false
                ? 'Turn them on to comment from the shade'
                : 'Comment and Threads are in the shade',
          ),
          trailing: _Value(
            switch (_notifications) {
              true => 'On',
              false => 'Off',
              null => '',
            },
            key: const ValueKey('notifications-state'),
          ),
        ),
        ListTile(
          title: const Text('Open settings'),
          subtitle: const Text('Clax’s page in Android settings'),
          onTap: () => unawaited(_openAppSettings()),
        ),
        _SwitchRow(
          title: 'Show the Comment tab',
          subtitle: 'Also while notifications are on',
          value: s.showCommentTab,
          onChanged: (v) => _change((s) => s.showCommentTab = v),
        ),
      ],
      if (widget.home case final h?) ...[
        const _Header('Home screen'),
        _HomeSection(controls: h),
      ],
      const _Header('About'),
      ListTile(
        title: const Text('Clax'),
        trailing: _Value('Version ${widget.version}'),
      ),
      if (_homeVersion case final v?)
        ListTile(
          title: const Text('Home screen'),
          trailing: _Value('Version $v'),
        ),
      ListTile(
        key: const Key('licences'),
        title: const Text('Licences'),
        subtitle: const Text('Clax is free software under the GPL, version 2'),
        onTap: () => showClaxLicences(context),
      ),
      if (_haiku case final h?)
        Padding(
          padding: const EdgeInsets.fromLTRB(
            ClaxSpace.s16,
            ClaxSpace.s12,
            ClaxSpace.s16,
            ClaxSpace.s8,
          ),
          child: Align(
            alignment: AlignmentDirectional.centerStart,
            child: HaikuLine(text: h, onAnother: _anotherHaiku),
          ),
        ),
      const _Header('Developer'),
      _SwitchRow(
        title: 'Developer tools',
        value: s.developer,
        onChanged: (v) {
          DeveloperScope.set(context, v);
          _change((s) => s.developer = v);
        },
      ),
      if (s.developer) ...[
        const _Header('Recent timings'),
        ..._timings(context),
      ],
    ];
  }

  /// Whether requests may go to Anthropic; Turn off takes an Allow back.
  Widget _aiConsentTile(AppSettings s) {
    final allowed = s.aiConsent == true;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        ListTile(
          title: const Text('Requests to Anthropic'),
          subtitle: Text(allowed ? aiConsentAllowed : aiConsentAsks),
        ),
        if (allowed)
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: ClaxSpace.s16),
            child: Wrap(
              alignment: WrapAlignment.end,
              children: [
                ClaxButton(
                  key: const ValueKey('ai-consent-turn-off'),
                  label: 'Turn off',
                  onPressed: () => _change((s) => s.aiConsent = false),
                ),
              ],
            ),
          ),
      ],
    );
  }

  Widget _keyTile(BuildContext context) {
    if (_hasKey && !_editingKey) {
      // The actions sit under the row, so the title keeps its width at
      // large text sizes.
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          const ListTile(
            title: Text('Claude API key'),
            subtitle: Text('A key is saved'),
          ),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: ClaxSpace.s16),
            child: Wrap(
              alignment: WrapAlignment.end,
              spacing: ClaxSpace.s8,
              children: [
                ClaxButton(
                  label: 'Replace',
                  onPressed: () => setState(() => _editingKey = true),
                ),
                ClaxButton(
                  label: 'Remove',
                  kind: ClaxButtonKind.destructive,
                  onPressed: () => _writeKey(null),
                ),
              ],
            ),
          ),
        ],
      );
    }
    return _Inset(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          ClaxTextField(
            key: const Key('api-key-field'),
            controller: _key,
            label: 'Claude API key',
            obscure: !_showKey,
            autocorrect: false,
            keyboardType: TextInputType.visiblePassword,
            helper: 'Create one at console.anthropic.com, then paste it here',
            error: _keyError,
            suffix: Row(
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
            onChanged: (_) => setState(() => _keyError = null),
            onSubmitted: (_) => _saveKey(),
          ),
          const SizedBox(height: ClaxSpace.s8),
          Wrap(
            alignment: WrapAlignment.end,
            spacing: ClaxSpace.s8,
            children: [
              if (_editingKey)
                ClaxButton(
                  label: 'Keep the saved key',
                  onPressed: () => setState(() {
                    _editingKey = false;
                    _key.clear();
                  }),
                ),
              ClaxButton(
                label: 'Save key',
                kind: ClaxButtonKind.tonal,
                onPressed: _key.text.trim().isEmpty ? null : _saveKey,
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

/// Open in comment mode, Undo last change, Reset to Clax's home screen
/// (each needs a home app; Undo and Reset a [HomeRecovery] too), and Use
/// the built-in home screen, saved as [HomeSettings.builtin].
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
          title: const Text('Open in comment mode'),
          subtitle: const Text('Comment on your home screen to change it'),
          enabled: has,
          onTap: _c.open,
        ),
        ListTile(
          title: const Text('Undo last change'),
          subtitle: const Text('Go back to the version before the latest'),
          enabled: can,
          onTap: () => _run((r) => r.undoLast(), 'Undid the last change'),
        ),
        ListTile(
          title: const Text('Reset to Clax’s home screen'),
          subtitle: const Text(
            'Undoes your changes to the home screen. Your apps, stars and '
            'folders stay.',
          ),
          enabled: can,
          onTap: () => _run((r) => r.reset(), 'Reset to Clax’s home screen'),
        ),
        _SwitchRow(
          title: 'Use the built-in home screen',
          subtitle: 'Until you switch this off',
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
  sendFirstStep => 'Send → first step',
  _ => name,
};

/// [d] in seconds with one decimal, as `8.4 s`.
String seconds(Duration d) =>
    '${(d.inMilliseconds / 1000).toStringAsFixed(1)} s';

/// A settings row with a switch in its family's form ([ClaxSwitch]) at its
/// trailing end: [title], [subtitle] under it in `muted`, and a press
/// anywhere on the row toggles it. It is read as one toggle.
class _SwitchRow extends StatelessWidget {
  const _SwitchRow({
    required this.title,
    this.subtitle,
    required this.value,
    required this.onChanged,
  });

  final String title;
  final String? subtitle;
  final bool value;

  /// Called with the new value; null disables the row.
  final ValueChanged<bool>? onChanged;

  @override
  Widget build(BuildContext context) {
    final change = onChanged;
    return MergeSemantics(
      child: ListTile(
        title: Text(title),
        subtitle: subtitle == null ? null : Text(subtitle!),
        enabled: change != null,
        trailing: ClaxSwitch(value: value, onChanged: change),
        onTap: change == null ? null : () => change(!value),
      ),
    );
  }
}

/// A row in the column's 16 dp inset, for a field.
class _Inset extends StatelessWidget {
  const _Inset({required this.child});
  final Widget child;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.fromLTRB(
      ClaxSpace.s16,
      ClaxSpace.s4,
      ClaxSpace.s16,
      ClaxSpace.s8,
    ),
    child: child,
  );
}

/// A row's value at its trailing end, in `muted`.
class _Value extends StatelessWidget {
  const _Value(this.text, {super.key});
  final String text;

  @override
  Widget build(BuildContext context) => Text(
    text,
    style: Theme.of(
      context,
    ).textTheme.bodyMedium!.copyWith(color: claxPalette(context).muted),
  );
}

/// A row naming one choice of several: [title], the chosen [value] at its
/// trailing end, and a press opens [items] as a menu under it; the chosen
/// item's ID goes to [onChosen].
class _MenuRow extends StatelessWidget {
  const _MenuRow({
    required this.title,
    required this.value,
    required this.items,
    required this.onChosen,
  });

  final String title, value;
  final List<ClaxMenuItem> items;
  final ValueChanged<String> onChosen;

  Future<void> _open(BuildContext context) async {
    final box = context.findRenderObject()! as RenderBox;
    final at = box.localToGlobal(Offset.zero) & box.size;
    final id = await ClaxMenu.show(context, items: items, anchor: at);
    if (id != null) onChosen(id);
  }

  @override
  Widget build(BuildContext context) => Builder(
    builder: (context) => ListTile(
      title: Text(title),
      trailing: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Flexible(child: _Value(value)),
          const SizedBox(width: ClaxSpace.s4),
          Icon(Icons.unfold_more_rounded, color: claxPalette(context).muted),
        ],
      ),
      onTap: () => _open(context),
    ),
  );
}

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
