import 'dart:async';
import 'dart:math' show Random;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../agent/timings.dart';
import '../home/home_recovery.dart';
import '../home/home_settings.dart';
import '../platform/secure_key.dart';
import '../platform/settings_store.dart';
import '../remote/models.dart';
import '../remote/remote_api.dart';
import '../version.dart';
import 'ai_consent_sheet.dart';
import 'developer.dart';
import 'haiku.dart';
import 'kit/kit.dart';
import 'licences.dart';
import 'settings/settings_tabs.dart';
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
/// - Sharing: with a [remote], clax.dev (Connect, which runs [connect];
///   once connected, "Connected as `<name>`" with Disconnect, and
///   "Publishing as", the account published apps show); the name bundles
///   carry and the sharing key's fingerprint;
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
/// With a [tab] it shows only that tab's section, as the Mac's Settings
/// window does (`SettingsTabs`): in the Apple family a grouped form, each
/// row's label right-aligned in a 180 point column and its controls beside
/// it; in the Material family the section's rows. About is the General
/// tab, Developer the Advanced one, and the Comment tab's switch is the
/// phone's alone.
///
/// Every choice is saved to [store] as it is made, and [onSaved] told. The
/// key is saved only through [secureKey], on Save key, and is never shown
/// back: a saved key reads "A key is saved", with Replace and Remove.
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
    this.tab,
    this.onSaved,
    this.remote,
    this.connect,
  });

  /// The hosted Clax remotes, for Sharing's clax.dev rows; none without.
  final RemoteApi? remote;

  /// Signs this device in to clax.dev (the connect sheet), answering the
  /// remote once connected.
  final Future<Remote?> Function(BuildContext context)? connect;

  /// The remote Sharing offers to connect to.
  static const remoteHost = 'clax.dev';

  /// The one section to show, as a Settings window's tab; null shows them
  /// all in one column.
  final SettingsTab? tab;

  /// Told after each choice or key is saved.
  final VoidCallback? onSaved;

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

  /// Whether notifications are on for Clax (`AppNotice.allowed` on
  /// Android, the system setting on the Mac); null where Clax cannot tell,
  /// and then there is no Notifications section.
  final Future<bool> Function()? notificationsAllowed;

  /// The channel Open settings goes through: `openAppSettings` opens
  /// Clax's page of the system settings (Android's app page, the Mac's
  /// Notifications pane).
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

  /// The remotes this device is signed in to; null before the first read.
  List<Remote>? _remotes;

  /// Whether a connect or a disconnect is going on.
  var _remoteBusy = false;
  AppLifecycleListener? _lifecycle;

  @override
  void initState() {
    super.initState();
    _init();
    _loadHaikus();
    if (widget.remote != null) unawaited(_loadRemotes());
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

  Future<void> _loadRemotes() async {
    List<Remote> all;
    try {
      all = await widget.remote!.remotes();
    } on Object {
      all = const [];
    }
    if (mounted) setState(() => _remotes = all);
  }

  /// The remote this device publishes to: the connected one at
  /// [SettingsScreen.remoteHost], else the first connected; null with
  /// none.
  Remote? get _connected {
    final on = [
      for (final r in _remotes ?? const <Remote>[])
        if (r.connected) r,
    ];
    return on.where((r) => r.name == SettingsScreen.remoteHost).firstOrNull ??
        on.firstOrNull;
  }

  Future<void> _connectRemote(BuildContext context) async {
    final connect = widget.connect;
    if (connect == null || _remoteBusy) return;
    setState(() => _remoteBusy = true);
    try {
      await connect(context);
    } finally {
      if (mounted) setState(() => _remoteBusy = false);
    }
    await _loadRemotes();
  }

  Future<void> _disconnect(BuildContext context, Remote r) async {
    final yes = await ClaxDialog.show(
      context,
      title: 'Disconnect from ${r.name}?',
      body:
          'Apps you published stay on ${r.name}, and their links keep '
          'working. Apps here stop publishing and updating until you '
          'connect again.',
      actions: const [
        ClaxDialogAction('Cancel'),
        ClaxDialogAction('Disconnect', kind: ClaxButtonKind.destructive),
      ],
      cancelIndex: 0,
    );
    if (yes != 1 || !mounted || !context.mounted) return;
    setState(() => _remoteBusy = true);
    final messenger = ScaffoldMessenger.maybeOf(context);
    var revoked = true;
    try {
      revoked = await widget.remote!.removeRemote(r.id);
    } on Object {
      messenger?.showSnackBar(
        SnackBar(content: Text('Couldn’t disconnect from ${r.name}')),
      );
    } finally {
      if (mounted) setState(() => _remoteBusy = false);
    }
    await _loadRemotes();
    if (!revoked && mounted && context.mounted) {
      // Forgotten here, but its token still works there.
      await ClaxDialog.show(
        context,
        title: 'Disconnected here only',
        body:
            '${r.name} still lists this device, and it can still act as '
            'you there. Remove it on ${r.name}’s People page.',
        actions: const [ClaxDialogAction('OK', kind: ClaxButtonKind.primary)],
        cancelIndex: 0,
      );
    }
  }

  /// "Connected as `<name>`", or what connecting does.
  String _remoteLine(Remote? r) => switch (r) {
    Remote(:final personName?) => 'Connected as $personName',
    Remote() => 'Connected',
    null => 'Publish your apps there and get them on your other devices',
  };

  Widget _remoteButton(BuildContext context, Remote? r) => r == null
      ? ClaxButton(
          key: const ValueKey('remote-connect'),
          label: 'Connect',
          kind: ClaxButtonKind.tonal,
          onPressed: _remoteBusy || widget.connect == null
              ? null
              : () => unawaited(_connectRemote(context)),
        )
      : ClaxButton(
          key: const ValueKey('remote-disconnect'),
          label: 'Disconnect',
          onPressed: _remoteBusy
              ? null
              : () => unawaited(_disconnect(context, r)),
        );

  static const _publishingAsLine =
      'The account your published apps show, not Your name';

  /// Sharing's clax.dev rows; none without a [SettingsScreen.remote] or
  /// before its first read.
  List<Widget> _remoteRows(BuildContext context) {
    if (widget.remote == null || _remotes == null) return const [];
    final r = _connected;
    return [
      ListTile(
        key: const ValueKey('remote-row'),
        title: Text(r?.name ?? SettingsScreen.remoteHost),
        subtitle: Text(_remoteLine(r)),
        trailing: _remoteButton(context, r),
      ),
      if (r?.personName case final name?)
        ListTile(
          key: const ValueKey('publishing-as'),
          title: const Text('Publishing as'),
          subtitle: const Text(_publishingAsLine),
          trailing: _Value(name),
        ),
    ];
  }

  /// Sharing's clax.dev rows in the Apple family's form.
  List<Widget> _remoteForm(BuildContext context) {
    if (widget.remote == null || _remotes == null) return const [];
    final r = _connected;
    final muted = _muted(context);
    return [
      _FormRow(
        label: r?.name ?? SettingsScreen.remoteHost,
        child: Column(
          key: const ValueKey('remote-row'),
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            _FormText(_remoteLine(r)),
            const SizedBox(height: ClaxSpace.s8),
            _remoteButton(context, r),
          ],
        ),
      ),
      if (r?.personName case final name?)
        _FormRow(
          label: 'Publishing as',
          child: Column(
            key: const ValueKey('publishing-as'),
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _FormText(name),
              const SizedBox(height: ClaxSpace.s4),
              Text(_publishingAsLine, style: muted),
            ],
          ),
        ),
    ];
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
    widget.onSaved?.call();
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
    widget.onSaved?.call();
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
    if (widget.tab case final tab?) return _tabBody(context, s, tab);
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

  /// The phone's sections, in order.
  static const _phoneOrder = [
    SettingsTab.claude,
    SettingsTab.dictation,
    SettingsTab.sharing,
    SettingsTab.notifications,
    SettingsTab.homeScreen,
    SettingsTab.general,
    SettingsTab.advanced,
  ];

  /// [tab]'s header on the phone.
  static String _phoneHeader(SettingsTab tab) => switch (tab) {
    SettingsTab.general => 'About',
    SettingsTab.homeScreen => 'Home screen',
    SettingsTab.advanced => 'Developer',
    _ => tab.title,
  };

  List<Widget> _rows(BuildContext context, AppSettings s) => [
    for (final t in _phoneOrder)
      if (_section(context, s, t, phone: true) case final rows
          when rows.isNotEmpty) ...[_Header(_phoneHeader(t)), ...rows],
  ];

  /// One tab's section: the phone's rows of it in the Material family, the
  /// grouped form in the Apple family.
  Widget _tabBody(BuildContext context, AppSettings? s, SettingsTab tab) =>
      Material(
        type: MaterialType.transparency,
        child: s == null
            ? const SizedBox(height: 160)
            : _tabContent(context, s, tab),
      );

  Widget _tabContent(BuildContext context, AppSettings s, SettingsTab tab) {
    if (ClaxFamily.of(context) != ClaxFamily.apple) {
      return Padding(
        padding: const EdgeInsets.symmetric(vertical: ClaxSpace.s16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: _section(context, s, tab, phone: false),
        ),
      );
    }
    return Padding(
      padding: const EdgeInsets.fromLTRB(
        ClaxSpace.s20,
        ClaxSpace.s20,
        ClaxSpace.s32,
        ClaxSpace.s24,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: _form(context, s, tab),
      ),
    );
  }

  /// [tab]'s rows in one column; [phone] adds the rows only the phone has
  /// (the Comment tab's switch).
  List<Widget> _section(
    BuildContext context,
    AppSettings s,
    SettingsTab tab, {
    required bool phone,
  }) {
    switch (tab) {
      case SettingsTab.claude:
        final model = _model(s);
        final cli = s.backend == backendClaudeCli;
        return [
          _keyTile(context),
          if (widget.cliSupported) ...[
            _MenuRow(
              title: 'Agent',
              value: _agentWord(cli),
              items: _agentItems(cli),
              onChosen: (v) => _change((s) => s.backend = v),
            ),
            if (cli) _Inset(child: _pathField()),
          ],
          _MenuRow(
            title: 'Model',
            value: model.word,
            items: _modelItems(model),
            onChosen: (v) => _change((s) => s.model = v),
          ),
          if (widget.asksAiConsent) _aiConsentTile(s),
        ];
      case SettingsTab.dictation:
        return [
          _SwitchRow(
            title: 'Start dictation automatically',
            subtitle: _autoDictateDetail,
            value: s.dictateComments,
            onChanged: (v) => _change((s) => s.dictateComments = v),
          ),
          _SwitchRow(
            title: 'Dictation sounds and vibration',
            subtitle: 'A tone and a buzz as listening starts and stops',
            value: s.dictationCues,
            onChanged: (v) => _change((s) => s.dictationCues = v),
          ),
        ];
      case SettingsTab.sharing:
        return [
          ..._remoteRows(context),
          _Inset(child: _nameField(showLabel: true)),
          if (_fingerprint case final f?)
            ListTile(
              title: const Text('Your sharing key'),
              subtitle: _fingerprintLine(f),
            )
          else
            const ListTile(subtitle: Text(_noSharingKey)),
        ];
      case SettingsTab.notifications:
        if (widget.notificationsAllowed == null) return const [];
        return [
          ListTile(
            title: const Text('Notifications for Clax'),
            subtitle: Text(
              _notifications == false
                  ? 'Turn them on to comment from the shade'
                  : 'Comment and Threads are in the shade',
            ),
            trailing: _Value(
              _notificationsWord,
              key: const ValueKey('notifications-state'),
            ),
          ),
          ListTile(
            title: const Text('Open settings'),
            subtitle: const Text('Clax’s page in Android settings'),
            onTap: () => unawaited(_openAppSettings()),
          ),
          if (phone)
            _SwitchRow(
              title: 'Show the Comment tab',
              subtitle: 'Also while notifications are on',
              value: s.showCommentTab,
              onChanged: (v) => _change((s) => s.showCommentTab = v),
            ),
        ];
      case SettingsTab.homeScreen:
        return [
          if (widget.home case final h?) _HomeSection(controls: h),
        ];
      case SettingsTab.general:
        return [
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
            subtitle: const Text(_licenceLine),
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
        ];
      case SettingsTab.advanced:
        return [
          _SwitchRow(
            title: 'Developer tools',
            value: s.developer,
            onChanged: (v) => _setDeveloper(context, v),
          ),
          if (s.developer) ...[
            const _Header('Recent timings'),
            ..._timings(context),
          ],
        ];
    }
  }

  /// [tab]'s grouped form, for the Apple family.
  List<Widget> _form(BuildContext context, AppSettings s, SettingsTab tab) {
    final muted = _muted(context);
    switch (tab) {
      case SettingsTab.general:
        return [
          _FormRow(label: 'Clax', child: _FormText('Version ${widget.version}')),
          if (_homeVersion case final v?)
            _FormRow(label: 'Home screen', child: _FormText('Version $v')),
          _FormRow(
            label: 'Licences',
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                ClaxButton(
                  key: const Key('licences'),
                  label: 'Show licences',
                  kind: ClaxButtonKind.tonal,
                  onPressed: () => showClaxLicences(context),
                ),
                const SizedBox(height: ClaxSpace.s4),
                Text(_licenceLine, style: muted),
              ],
            ),
          ),
          if (_haiku case final h?)
            _FormRow(
              child: Padding(
                padding: const EdgeInsets.only(top: ClaxSpace.s8),
                child: HaikuLine(text: h, onAnother: _anotherHaiku),
              ),
            ),
        ];
      case SettingsTab.claude:
        final model = _model(s);
        final cli = s.backend == backendClaudeCli;
        return [
          _FormRow(label: 'API key', child: _keyForm(context)),
          if (widget.cliSupported) ...[
            _FormRow(
              label: 'Agent',
              child: _PopUp(
                value: _agentWord(cli),
                items: _agentItems(cli),
                onChosen: (v) => _change((s) => s.backend = v),
              ),
            ),
            if (cli)
              _FormRow(label: 'Claude Code path', child: _pathField(showLabel: false)),
          ],
          _FormRow(
            label: 'Model',
            child: _PopUp(
              value: model.word,
              items: _modelItems(model),
              onChosen: (v) => _change((s) => s.model = v),
            ),
          ),
          if (widget.asksAiConsent)
            _FormRow(
              label: 'Requests to Anthropic',
              child: _aiConsentForm(context, s),
            ),
        ];
      case SettingsTab.dictation:
        return [
          _FormRow(
            label: 'Dictation',
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                ClaxCheckbox(
                  value: s.dictateComments,
                  label: 'Start dictation automatically',
                  detail: _autoDictateDetail,
                  onChanged: (v) => _change((s) => s.dictateComments = v),
                ),
                const SizedBox(height: ClaxSpace.s8),
                ClaxCheckbox(
                  value: s.dictationCues,
                  label: 'Play sounds',
                  detail: 'A tone as listening starts and stops',
                  onChanged: (v) => _change((s) => s.dictationCues = v),
                ),
              ],
            ),
          ),
        ];
      case SettingsTab.sharing:
        return [
          ..._remoteForm(context),
          _FormRow(label: 'Your name', child: _nameField(showLabel: false)),
          _FormRow(
            label: 'Sharing key',
            child: switch (_fingerprint) {
              final f? => _fingerprintLine(f),
              null => Text(_noSharingKey, style: muted),
            },
          ),
        ];
      case SettingsTab.notifications:
        return [
          _FormRow(
            label: 'Notifications',
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                _FormText(
                  _notificationsWord,
                  key: const ValueKey('notifications-state'),
                ),
                const SizedBox(height: ClaxSpace.s4),
                Text(
                  _notifications == false
                      ? 'Turn them on for the reminders apps set'
                      : 'Apps you allow can remind you',
                  style: muted,
                ),
                const SizedBox(height: ClaxSpace.s8),
                ClaxButton(
                  label: 'Open System Settings',
                  kind: ClaxButtonKind.tonal,
                  onPressed: () => unawaited(_openAppSettings()),
                ),
              ],
            ),
          ),
        ];
      case SettingsTab.homeScreen:
        return [
          if (widget.home case final h?) _HomeSection(controls: h, form: true),
        ];
      case SettingsTab.advanced:
        return [
          _FormRow(
            label: 'Developer',
            child: ClaxCheckbox(
              value: s.developer,
              label: 'Developer tools',
              detail: 'Timings, the inspector’s code and data, and raw errors',
              onChanged: (v) => _setDeveloper(context, v),
            ),
          ),
          if (s.developer)
            _FormRow(
              label: 'Recent timings',
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: _formTimings(context),
              ),
            ),
        ];
    }
  }

  static const _autoDictateDetail =
      'Listen as soon as you start a new app or a comment';
  static const _noSharingKey =
      'A sharing key is made the first time you share an app';
  static const _licenceLine = 'Clax is free software under the GPL, version 2';

  String get _notificationsWord => switch (_notifications) {
    true => 'On',
    false => 'Off',
    null => '',
  };

  TextStyle _muted(BuildContext context) => Theme.of(
    context,
  ).textTheme.bodySmall!.copyWith(color: claxPalette(context).muted);

  void _setDeveloper(BuildContext context, bool v) {
    DeveloperScope.set(context, v);
    _change((s) => s.developer = v);
  }

  ModelChoice _model(AppSettings s) =>
      modelChoices.firstWhere(
        (m) => m.id == s.model,
        orElse: () => modelChoices.firstWhere((m) => m.id == defaultModel),
      );

  List<ClaxMenuItem> _modelItems(ModelChoice model) => [
    for (final m in modelChoices)
      ClaxMenuItem(m.id, m.word, detail: m.name, checked: m == model),
  ];

  static String _agentWord(bool cli) =>
      cli ? 'Claude Code on this Mac' : 'Claude API';

  static List<ClaxMenuItem> _agentItems(bool cli) => [
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
  ];

  Widget _pathField({bool showLabel = true}) => ClaxTextField(
    controller: _path,
    label: 'Claude Code path',
    hint: 'Found on PATH when empty',
    autocorrect: false,
    showLabel: showLabel,
    onChanged: (v) =>
        _change((s) => s.claudePath = v.trim().isEmpty ? null : v.trim()),
  );

  Widget _nameField({required bool showLabel}) => ClaxTextField(
    key: const Key('author-name-field'),
    controller: _name,
    label: 'Your name',
    helper: 'Shown to people you share apps with',
    maxLength: maxAuthorName,
    textCapitalization: TextCapitalization.words,
    showLabel: showLabel,
    onChanged: (v) => _change((s) => s.authorName = cleanAuthorName(v)),
  );

  Widget _fingerprintLine(String f) => Row(
    children: [
      const Icon(Icons.fingerprint_rounded, size: 18),
      const SizedBox(width: ClaxSpace.s8),
      Flexible(
        child: SelectableText(
          f,
          style: const TextStyle(fontFeatures: [FontFeature.tabularFigures()]),
        ),
      ),
    ],
  );

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
          _keyField(showLabel: true),
          const SizedBox(height: ClaxSpace.s8),
          _keyButtons(),
        ],
      ),
    );
  }

  /// The API key in the Apple family's form: the saved key's state with
  /// Replace and Remove, or the field with Save key.
  Widget _keyForm(BuildContext context) {
    if (_hasKey && !_editingKey) {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const _FormText('A key is saved'),
          const SizedBox(height: ClaxSpace.s8),
          Wrap(
            spacing: ClaxSpace.s8,
            children: [
              ClaxButton(
                label: 'Replace',
                kind: ClaxButtonKind.tonal,
                onPressed: () => setState(() => _editingKey = true),
              ),
              ClaxButton(
                label: 'Remove',
                kind: ClaxButtonKind.destructive,
                onPressed: () => _writeKey(null),
              ),
            ],
          ),
        ],
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _keyField(showLabel: false),
        const SizedBox(height: ClaxSpace.s8),
        _keyButtons(),
      ],
    );
  }

  Widget _keyField({required bool showLabel}) => ClaxTextField(
            key: const Key('api-key-field'),
            controller: _key,
            label: 'Claude API key',
            showLabel: showLabel,
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
          );

  Widget _keyButtons() => Wrap(
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
          );

  /// Whether requests may go to Anthropic, in the Apple family's form.
  Widget _aiConsentForm(BuildContext context, AppSettings s) {
    final allowed = s.aiConsent == true;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _FormText(allowed ? aiConsentAllowed : aiConsentAsks),
        if (allowed) ...[
          const SizedBox(height: ClaxSpace.s8),
          ClaxButton(
            key: const ValueKey('ai-consent-turn-off'),
            label: 'Turn off',
            kind: ClaxButtonKind.tonal,
            onPressed: () => _change((s) => s.aiConsent = false),
          ),
        ],
      ],
    );
  }

  /// The last ten timings as the Apple family's form lists them: each
  /// name, when, and its seconds.
  List<Widget> _formTimings(BuildContext context) {
    final recent = widget.timings.take(10).toList();
    final muted = _muted(context);
    if (recent.isEmpty) return [Text('Nothing measured yet', style: muted)];
    return [
      for (final t in recent)
        Padding(
          padding: const EdgeInsets.only(bottom: ClaxSpace.s4),
          child: Row(
            children: [
              Expanded(child: _FormText(timingLabel(t.name))),
              Text(_when(t.at.toLocal()), style: muted),
              const SizedBox(width: ClaxSpace.s12),
              _FormText(seconds(t.elapsed), tabular: true),
            ],
          ),
        ),
    ];
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
  const _HomeSection({required this.controls, this.form = false});
  final HomeScreenControls controls;

  /// Whether it is drawn as the Apple family's grouped form.
  final bool form;

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
    if (widget.form) return _form(context, has: has, can: can);
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

extension on _HomeSectionState {
  Widget _form(BuildContext context, {required bool has, required bool can}) {
    final muted = Theme.of(
      context,
    ).textTheme.bodySmall!.copyWith(color: claxPalette(context).muted);
    Widget action(String label, String detail, VoidCallback? run) => Padding(
      padding: const EdgeInsets.only(bottom: ClaxSpace.s12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          ClaxButton(label: label, kind: ClaxButtonKind.tonal, onPressed: run),
          const SizedBox(height: ClaxSpace.s4),
          Text(detail, style: muted),
        ],
      ),
    );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _FormRow(
          label: 'Home screen',
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              action(
                'Open in comment mode',
                'Comment on your home screen to change it',
                has ? _c.open : null,
              ),
              action(
                'Undo last change',
                'Go back to the version before the latest',
                can
                    ? () => _run((r) => r.undoLast(), 'Undid the last change')
                    : null,
              ),
              action(
                'Reset to Clax’s home screen',
                'Undoes your changes to the home screen. Your apps, stars '
                    'and folders stay.',
                can
                    ? () => _run(
                        (r) => r.reset(),
                        'Reset to Clax’s home screen',
                      )
                    : null,
              ),
            ],
          ),
        ),
        _FormRow(
          child: ClaxCheckbox(
            value: _c.settings.builtin,
            label: 'Use the built-in home screen',
            detail: 'Until you switch this off',
            onChanged: _builtin,
          ),
        ),
      ],
    );
  }
}

/// A row of the Apple family's grouped form: [label] right-aligned in a
/// 180 point column, [child] beside it; without a label the column is
/// left empty, so the row's controls line up under the ones above.
class _FormRow extends StatelessWidget {
  const _FormRow({this.label, required this.child});

  final String? label;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final text = label;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: ClaxSpace.s8),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            key: const ValueKey('settings-form-label'),
            width: 180,
            child: text == null
                ? null
                : Padding(
                    // On the first line of the control beside it.
                    padding: const EdgeInsets.only(top: 3),
                    child: Text(
                      text,
                      textAlign: TextAlign.end,
                      style: ClaxType.of(
                        ClaxRole.body,
                        Theme.of(context).platform,
                      ).copyWith(color: claxPalette(context).fg),
                    ),
                  ),
          ),
          const SizedBox(width: ClaxSpace.s12),
          Expanded(
            child: Align(alignment: AlignmentDirectional.topStart, child: child),
          ),
        ],
      ),
    );
  }
}

/// A value in the Apple family's form, in `body`.
class _FormText extends StatelessWidget {
  const _FormText(this.text, {super.key, this.tabular = false});

  final String text;
  final bool tabular;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(top: 3),
    child: Text(
      text,
      style: ClaxType.of(ClaxRole.body, Theme.of(context).platform).copyWith(
        color: claxPalette(context).fg,
        fontFeatures: tabular ? const [FontFeature.tabularFigures()] : null,
      ),
    ),
  );
}

/// A pop-up button in the Apple family's form: the chosen [value] with
/// the up-and-down chevrons, opening [items] as a menu under it; the
/// chosen item's ID goes to [onChosen].
class _PopUp extends StatelessWidget {
  const _PopUp({
    required this.value,
    required this.items,
    required this.onChosen,
  });

  final String value;
  final List<ClaxMenuItem> items;
  final ValueChanged<String> onChosen;

  @override
  Widget build(BuildContext context) {
    final p = claxPalette(context);
    final type = ClaxType.of(ClaxRole.body, Theme.of(context).platform);
    return Builder(
      builder: (context) => Semantics(
        button: true,
        value: value,
        child: Material(
          color: p.card,
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(ClaxRadius.controlApple),
            side: BorderSide(color: p.borderStrong),
          ),
          child: InkWell(
            borderRadius: BorderRadius.circular(ClaxRadius.controlApple),
            onTap: () async {
              final box = context.findRenderObject()! as RenderBox;
              final at = box.localToGlobal(Offset.zero) & box.size;
              final id = await ClaxMenu.show(context, items: items, anchor: at);
              if (id != null) onChosen(id);
            },
            child: Padding(
              padding: const EdgeInsets.fromLTRB(8, 3, 4, 3),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Flexible(
                    child: Text(value, style: type.copyWith(color: p.fg)),
                  ),
                  const SizedBox(width: ClaxSpace.s8),
                  Icon(Icons.unfold_more_rounded, size: 14, color: p.muted),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// A timing's name as Settings shows it.
String timingLabel(String name) => switch (name) {
  sayUsable => 'Say → usable',
  longpressLanded => 'Long-press → landed',
  homeUsable => 'Launch → home usable',
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
