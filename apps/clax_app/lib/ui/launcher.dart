import 'dart:async';
import 'dart:typed_data';

import 'package:flutter/material.dart';

import '../agent/timings.dart';
import '../bundle/lineage_store.dart' show LineageEntry;
import '../clax/api.dart';
import '../platform/voice.dart';
import '../agent/plan.dart';
import '../agent/worker.dart';
import 'app_icon.dart';
import 'app_inspector.dart';
import 'build_plan.dart';
import 'bundle_sheets.dart';
import 'inbox_screen.dart';
import 'make_service.dart';
import 'making_screen.dart';
import 'new_sheet.dart';

/// The home screen: the owner's apps, newest first, live from the daemon's
/// events, and the New app button.
///
/// New opens the New sheet (listening at once when dictation is available);
/// a submitted request starts a make on [makes] (one of the launcher's own
/// over [make] when null) and shows it in the build view; the artifact ID
/// it returns goes to [handOver] (which puts the app's route in the build
/// view's place), else to [open] once the build view has closed. A request
/// the person has not made yet (the sheet closed, Cancel, or Edit after a
/// failure) is kept for the next New.
///
/// Each make going on (or failed) shows as a tile before the apps, with
/// its request, a ring (the plan's progress once there is one) and how it
/// stands; a tap shows its build view again, caught up. Back from the build
/// view leaves the make going.
///
/// The launcher follows `gallery` and `inbox` on [stream] (the app's
/// shared stream, which it never closes), or on a stream of its own when
/// none is given. Once subscribed it loads the list and
/// [ClaxApi.inboxSummary]; then each `gallery` `version` refreshes its
/// app's tile, each `artifact_deleted` removes one, and each `inbox` event
/// carries the unread count. A topic's `resync` loads it again, as does
/// coming back from the inbox for the count.
///
/// The top bar's inbox button shows the unread count and pushes
/// [InboxScreen] on the same stream; an item opened there goes to
/// [onOpenQuestion] (a question, when given), else to [onOpenTarget] with
/// its thread or version, else to [open].
///
/// A long press on a tile opens its menu: Open; Inspect, which pushes
/// the [AppInspector] with [inspectorPermissions] (and a version opened
/// from its history going to [onOpenTarget]); and, with [bundles], Share
/// app (Export… on the Mac), which the inspector's Overview offers too.
/// With [bundles] the top bar's overflow holds Import app….
///
/// With [build], a tile whose app's build goes shows how far it is (a ring
/// and "3/7"), the making screen is the build view, and the inspector's
/// Plan tab shows the app's plan.
class Launcher extends StatefulWidget {
  const Launcher({
    super.key,
    required this.api,
    required this.open,
    required this.make,
    required this.voice,
    this.openSettings,
    this.needsKey = false,
    this.onOpenTarget,
    this.onOpenQuestion,
    this.stream,
    this.timings,
    this.handOver,
    this.retryDelay,
    this.inspectorPermissions = const [],
    this.build,
    this.makes,
    this.icons,
    this.bundles,
    this.lineage,
  });

  final ClaxApi api;
  final void Function(String aid) open;
  final MakeApp make;
  final VoiceInput voice;

  /// Shows Settings; without it the launcher shows no settings button.
  final VoidCallback? openSettings;

  /// Whether the agent cannot run until an API key is added: the launcher
  /// then says so above the apps, with Add key opening Settings.
  final bool needsKey;

  /// Opens app [aid] from the inbox: at [thread] and [comment] (a reply) or at
  /// version [version]. When null, the app opens as [open] opens it.
  final void Function(
    String aid, {
    String? thread,
    int? version,
    String? comment,
  })?
  onOpenTarget;

  /// Shows question [qid] for an answer, from the inbox. When null, a
  /// question about an app opens that app through [onOpenTarget].
  final void Function(String qid)? onOpenQuestion;

  /// Where the making screen records say→first-output; null records
  /// nothing.
  final Timings? timings;

  /// Opens the new app [aid] in the making screen's place, from that
  /// screen's [context], with the last render's screenshot ([lastFrame])
  /// to show until the app's page is up (`MakingScreen.onMade`). When null,
  /// the making screen closes and [open] opens the app.
  final void Function(BuildContext context, String aid, Uint8List? lastFrame)?
  handOver;

  /// The app's shared stream; when null the launcher opens its own and
  /// closes it as it goes.
  final ClaxStream? stream;

  /// Waits between tries to subscribe a topic of the launcher's own stream
  /// again after the stream dropped it; [Future.delayed] when null.
  final Future<void> Function(Duration)? retryDelay;

  /// The permission kinds the app inspector lists beyond the db read
  /// grants.
  final List<PermissionSource> inspectorPermissions;

  /// The plans and builds; null shows none.
  final BuildLink? build;

  /// The makes going on; when null, the launcher keeps its own over
  /// [make].
  final MakeService? makes;

  /// The apps' icons, which a tile shows in its badge; when null, the
  /// launcher keeps its own in memory.
  final AppIcons? icons;

  /// Shares apps as `.clax` files and opens them; null offers neither.
  final BundleFlows? bundles;

  /// An app's lineage entry, for the inspector's sharing line; null shows
  /// none.
  final Future<LineageEntry?> Function(String aid)? lineage;

  @override
  State<Launcher> createState() => _LauncherState();
}

class _LauncherState extends State<Launcher> {
  List<ArtifactSummary>? _apps;
  Object? _loadError;
  String _draft = '';
  late final ClaxStream _stream;
  StreamSubscription<StreamEvent>? _events;
  int _unread = 0;
  int _summaries = 0;

  /// The topics being subscribed again after an `unsubscribed`.
  final Set<String> _resubscribing = {};

  /// Bumped by each load, so only the newest load's answer is kept.
  int _loads = 0;

  static const _topics = ['gallery', 'inbox'];

  late final MakeService _makes = widget.makes ?? MakeService(widget.make);

  late final AppIcons _icons = widget.icons ?? AppIcons(widget.api);

  @override
  void initState() {
    super.initState();
    _makes.addListener(_onMakes);
    _stream = widget.stream ?? widget.api.stream();
    _events = _stream.events.listen(_onEvent, onError: (Object _) {});
    unawaited(_watch());
  }

  void _onMakes() {
    if (mounted) setState(() {});
  }

  @override
  void dispose() {
    _makes.removeListener(_onMakes);
    if (widget.makes == null) _makes.dispose();
    if (widget.icons == null) _icons.dispose();
    unawaited(_events?.cancel());
    if (widget.stream == null) unawaited(_stream.close());
    super.dispose();
  }

  Future<void> _watch() async {
    try {
      await _stream.subscribe(_topics);
    } catch (_) {
      // The list and count still load; they only miss live changes until
      // the next fetch.
    }
    if (!mounted) return;
    await Future.wait([_load(), _loadUnread()]);
  }

  /// Subscribes [topic] again after the launcher's own stream dropped it,
  /// then refetches what it shows.
  Future<void> _resubscribe(String topic) async {
    if (!_resubscribing.add(topic)) return;
    try {
      final ok = await resubscribe(
        _stream,
        [topic],
        delay: widget.retryDelay ?? Future.delayed,
        alive: () => mounted,
      );
      if (ok && mounted) await _reload(topic);
    } finally {
      _resubscribing.remove(topic);
    }
  }

  Future<void> _reload(String topic) async {
    if (topic == 'gallery') await _load();
    if (topic == 'inbox') await _loadUnread();
  }

  Future<void> _loadUnread() async {
    final load = ++_summaries;
    try {
      final summary = await widget.api.inboxSummary();
      if (mounted && load == _summaries) {
        setState(() => _unread = summary.unread);
      }
    } catch (_) {
      // Kept as it was; the next event or fetch corrects it.
    }
  }

  void _onEvent(StreamEvent e) {
    if (!mounted) return;
    final topic = e.data['topic'];
    switch (e.type) {
      case 'unsubscribed' when topic is String && _topics.contains(topic):
        unawaited(_resubscribe(topic));
      case 'resync' when topic is String:
        unawaited(_reload(topic));
      case 'version' || 'artifact_deleted' when e.topic == 'gallery':
        unawaited(_onGallery(e));
      case _ when e.topic == 'inbox':
        if (e.data['unread'] case final num n) {
          // A newer count than any summary still in flight.
          _summaries++;
          setState(() => _unread = n.toInt());
        }
    }
  }

  Future<void> _openInbox() async {
    await Navigator.of(context).push<void>(
      MaterialPageRoute(
        builder: (_) =>
            InboxScreen(api: widget.api, onOpen: _openItem, stream: _stream),
      ),
    );
    if (mounted) await _loadUnread();
  }

  void _openItem(InboxItem item) {
    final question = item.question;
    if (item.kind == 'question' &&
        question != null &&
        widget.onOpenQuestion != null) {
      widget.onOpenQuestion!(question.id);
      return;
    }
    final aid = item.artifactId;
    if (aid == null) return;
    final target = widget.onOpenTarget;
    if (target == null) {
      widget.open(aid);
      return;
    }
    target(
      aid,
      thread: item.kind == 'reply' ? item.thread?.id : null,
      version: item.kind == 'version' ? item.versionN : null,
      comment: item.kind == 'reply' ? item.replyCommentId : null,
    );
  }

  Future<void> _load() async {
    final load = ++_loads;
    try {
      final apps = await widget.api.listArtifacts();
      if (!mounted || load != _loads) return;
      setState(() {
        _apps = _newestFirst(apps);
        _loadError = null;
      });
    } catch (e) {
      if (mounted && load == _loads) setState(() => _loadError = e);
    }
  }

  static List<ArtifactSummary> _newestFirst(List<ArtifactSummary> apps) =>
      List.of(apps)..sort((a, b) => b.updatedAt.compareTo(a.updatedAt));

  Future<void> _onGallery(StreamEvent e) async {
    final aid = e.data['artifact_id'];
    if (aid is! String) return;
    if (e.type == 'artifact_deleted') {
      setState(() => _apps = _apps?.where((a) => a.id != aid).toList());
      return;
    }
    try {
      final summary = (await widget.api.getArtifact(aid)).artifact;
      if (!mounted) return;
      setState(() {
        _apps = _newestFirst([summary, ...?_apps?.where((a) => a.id != aid)]);
      });
    } catch (_) {
      // Deleted again, or the daemon is restarting; the next load catches up.
    }
  }

  Future<void> _newApp() async {
    final request = await showNewSheet(
      context,
      widget.voice,
      _draft,
      onChanged: (t) => _draft = t,
    );
    if (request == null || !mounted) return;
    _draft = request;
    // The build view starts it, once it is up.
    await _showMake(_makes.add(request));
  }

  /// Asks whether to make interrupted make [s] again (then shows it) or to
  /// discard it; closing the question leaves it as it is.
  Future<void> _offerAgain(MakeSession s) async {
    final again = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Make it again?'),
        content: Text('Clax closed before it finished “${s.request}”.'),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Discard'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('Make it again'),
          ),
        ],
      ),
    );
    if (!mounted || again == null) return;
    if (!again) {
      _makes.dismiss(s);
      return;
    }
    _makes.resume(s);
    await _showMake(s);
  }

  /// Shows make [s] in the build view, caught up with what it did.
  Future<void> _showMake(MakeSession s) async {
    final result = await Navigator.of(context).push<MakingResult>(
      MaterialPageRoute(
        fullscreenDialog: true,
        builder: (_) => MakingScreen(
          request: s.request,
          session: s,
          openSettings: widget.openSettings,
          timings: widget.timings,
          onMade: widget.handOver,
          build: widget.build,
        ),
      ),
    );
    if (!mounted) return;
    switch (result) {
      case Made(:final aid):
        if (_draft == s.request) _draft = '';
        if (widget.handOver == null) widget.open(aid);
      case MakingLeft():
        if (_draft == s.request) _draft = '';
      case EditRequest():
        _draft = s.request;
        await _newApp();
      case null:
        _draft = s.request;
    }
  }

  /// How far a going build is, for its tile; null when none goes.
  static ({int done, int total})? _building(Plan? plan, BuildState? state) {
    if (!buildGoing(state) || plan == null) return null;
    final c = planCount(plan);
    return c.total == 0 ? null : c;
  }

  /// Whether [aid]'s build stopped (stopped, capped or failed) with
  /// features still to build, so the menu offers Resume build.
  Future<bool> _resumable(String aid) async {
    final b = widget.build;
    if (b == null) return false;
    final Plan? plan;
    try {
      plan = await b.plans.get(aid);
    } on Object {
      return false;
    }
    if (plan == null || plan.todo.isEmpty) return false;
    final phase = b.control.buildOf(aid)?.phase;
    return plan.stopped ||
        phase == BuildPhase.stopped ||
        phase == BuildPhase.capped ||
        phase == BuildPhase.failed;
  }

  Future<void> _menu(ArtifactSummary app) async {
    final resume = await _resumable(app.id);
    if (!mounted) return;
    final choice = await showModalBottomSheet<String>(
      context: context,
      builder: (context) => SafeArea(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(24, 0, 24, 8),
              child: Text(
                app.title,
                style: Theme.of(context).textTheme.titleMedium,
              ),
            ),
            ListTile(
              leading: const Icon(Icons.open_in_new_rounded),
              title: const Text('Open'),
              onTap: () => Navigator.of(context).pop('open'),
            ),
            ListTile(
              key: const ValueKey('inspect'),
              leading: const Icon(Icons.manage_search_rounded),
              title: const Text('Inspect'),
              subtitle: const Text('Files, data and permissions'),
              onTap: () => Navigator.of(context).pop('inspect'),
            ),
            if (widget.bundles case final b?)
              ListTile(
                key: const ValueKey('share-app'),
                leading: Icon(
                  b.toFile ? Icons.save_alt_rounded : Icons.ios_share_rounded,
                ),
                title: Text(b.shareLabel),
                subtitle: Text(
                  b.toFile ? 'Save it as a .clax file' : 'Send it as a file',
                ),
                onTap: () => Navigator.of(context).pop('share'),
              ),
            if (resume)
              ListTile(
                key: const ValueKey('resume-build'),
                leading: const Icon(Icons.play_arrow_rounded),
                title: const Text('Resume build'),
                subtitle: const Text('Build the rest of its plan'),
                onTap: () => Navigator.of(context).pop('resume'),
              ),
            const SizedBox(height: 8),
          ],
        ),
      ),
    );
    if (!mounted) return;
    switch (choice) {
      case 'open':
        widget.open(app.id);
      case 'resume':
        await widget.build?.control.resumeBuild(app.id);
      case 'share':
        await widget.bundles?.share(context, app.id, svg: _icons.svgOf(app.id));
      case 'inspect':
        final target = widget.onOpenTarget;
        await Navigator.of(context).push<void>(
          MaterialPageRoute(
            builder: (_) => AppInspector(
              api: widget.api,
              aid: app.id,
              permissions: widget.inspectorPermissions,
              bundles: widget.bundles,
              lineage: widget.lineage,
              plan: switch (widget.build) {
                final b? => (context, aid) => BuildWatch(
                  link: b,
                  appId: aid,
                  builder: (context, plan, state) => plan == null
                      ? const _Message(
                          icon: Icons.checklist_rounded,
                          text: 'No plan',
                          hint:
                              'Apps built from a plan show their features '
                              'here.',
                        )
                      : BuildPlanPanel(
                          plan: plan,
                          state: state,
                          handle: false,
                          onResume: plan.todo.isEmpty
                              ? null
                              : () => unawaited(b.control.resumeBuild(aid)),
                        ),
                ),
                null => null,
              },
              onOpenVersion: target == null
                  ? null
                  : (aid, n) => target(aid, version: n),
            ),
          ),
        );
    }
  }

  @override
  Widget build(BuildContext context) {
    final apps = _apps;
    return Scaffold(
      floatingActionButton: FloatingActionButton.extended(
        onPressed: _newApp,
        icon: const Icon(Icons.mic_rounded),
        label: const Text('New app'),
      ),
      body: RefreshIndicator(
        onRefresh: _load,
        child: CustomScrollView(
          physics: const AlwaysScrollableScrollPhysics(),
          slivers: [
            SliverAppBar.large(
              title: const Text('Clax'),
              actions: [
                IconButton(
                  tooltip: 'Inbox',
                  onPressed: _openInbox,
                  icon: Badge.count(
                    count: _unread,
                    isLabelVisible: _unread > 0,
                    child: const Icon(Icons.inbox_outlined),
                  ),
                ),
                if (widget.openSettings != null)
                  IconButton(
                    tooltip: 'Settings',
                    onPressed: widget.openSettings,
                    icon: const Icon(Icons.settings_outlined),
                  ),
                if (widget.bundles case final b?)
                  PopupMenuButton<String>(
                    key: const ValueKey('launcher-more'),
                    tooltip: 'More',
                    onSelected: (_) => unawaited(b.importPicked(context)),
                    itemBuilder: (_) => const [
                      PopupMenuItem(
                        value: 'import',
                        child: Text('Import app…'),
                      ),
                    ],
                  ),
                const SizedBox(width: 4),
              ],
            ),
            if (widget.needsKey)
              SliverToBoxAdapter(child: _KeyBanner(onAdd: widget.openSettings)),
            if (apps == null && _loadError != null)
              SliverFillRemaining(
                hasScrollBody: false,
                child: _Message(
                  icon: Icons.cloud_off_rounded,
                  text: "Couldn't load your apps",
                  action: TextButton(
                    onPressed: _load,
                    child: const Text('Try again'),
                  ),
                ),
              )
            else if (apps == null)
              const SliverFillRemaining(
                hasScrollBody: false,
                child: SizedBox.shrink(),
              )
            else if (apps.isEmpty && _makes.makes.isEmpty)
              const SliverFillRemaining(
                hasScrollBody: false,
                child: _Message(
                  icon: Icons.auto_awesome_rounded,
                  text: 'Say what you want, and Clax makes it.',
                  hint: '“A tracker for my runs with a weekly total”',
                ),
              )
            else
              SliverPadding(
                padding: const EdgeInsets.fromLTRB(16, 8, 16, 112),
                sliver: SliverGrid.builder(
                  gridDelegate: SliverGridDelegateWithMaxCrossAxisExtent(
                    maxCrossAxisExtent: _AppTile.maxWidth,
                    mainAxisSpacing: 8,
                    crossAxisSpacing: 4,
                    mainAxisExtent: _AppTile.extent(context),
                  ),
                  itemCount: _makes.makes.length + apps.length,
                  itemBuilder: (context, i) {
                    final makes = _makes.makes;
                    if (i < makes.length) {
                      final m = makes[i];
                      return MakingTile(
                        key: ObjectKey(m),
                        session: m,
                        link: widget.build,
                        onTap: () => unawaited(
                          m.interrupted ? _offerAgain(m) : _showMake(m),
                        ),
                      );
                    }
                    i -= makes.length;
                    Widget tile(({int done, int total})? building) => _AppTile(
                      app: apps[i],
                      icons: _icons,
                      building: building,
                      onTap: () => widget.open(apps[i].id),
                      onLongPress: () => _menu(apps[i]),
                    );
                    final b = widget.build;
                    if (b == null) return tile(null);
                    return BuildWatch(
                      key: ValueKey('build-${apps[i].id}'),
                      link: b,
                      appId: apps[i].id,
                      builder: (context, plan, state) =>
                          tile(_building(plan, state)),
                    );
                  },
                ),
              ),
          ],
        ),
      ),
    );
  }
}

/// One cell of the launcher's home-screen grid: [icon] centred in a
/// rounded square of [_AppTile.iconSize], with [label] centred below it,
/// on no card of its own.
class _HomeCell extends StatelessWidget {
  const _HomeCell({
    super.key,
    required this.semantics,
    required this.icon,
    required this.label,
    required this.onTap,
    this.onLongPress,
  });

  final String semantics;
  final Widget icon;
  final Widget label;
  final VoidCallback onTap;
  final VoidCallback? onLongPress;

  @override
  Widget build(BuildContext context) => Semantics(
    button: true,
    label: semantics,
    excludeSemantics: true,
    child: InkWell(
      onTap: onTap,
      onLongPress: onLongPress,
      borderRadius: BorderRadius.circular(20),
      child: Padding(
        padding: const EdgeInsets.fromLTRB(4, _AppTile._top, 4, 4),
        child: Column(
          children: [
            SizedBox.square(dimension: _AppTile.iconSize, child: icon),
            const SizedBox(height: _AppTile._gap),
            label,
          ],
        ),
      ),
    ),
  );
}

/// The style of a launcher cell's label.
TextStyle _labelStyle(BuildContext context) =>
    Theme.of(context).textTheme.bodyMedium!.copyWith(height: 1.25);

/// A centred label of at most [lines] lines.
Widget _label(String text, TextStyle style, {int lines = 2}) => Text(
  text,
  maxLines: lines,
  overflow: TextOverflow.ellipsis,
  textAlign: TextAlign.center,
  style: style,
);

class _AppTile extends StatelessWidget {
  const _AppTile({
    required this.app,
    required this.onTap,
    required this.onLongPress,
    required this.icons,
    this.building,
  });

  /// The side of a cell's icon.
  static const iconSize = 60.0;

  /// The corner radius of a cell's icon.
  static const iconRadius = 15.0;

  static const _top = 8.0;
  static const _gap = 8.0;

  /// The widest a cell may be: four to a row on a phone, more on wider
  /// screens.
  static const maxWidth = 112.0;

  /// A cell's height: the icon, a gap and two lines of the label at the
  /// current text scale, so a wrapped title always fits.
  static double extent(BuildContext context) {
    final style = _labelStyle(context);
    final size = MediaQuery.textScalerOf(context).scale(style.fontSize!);
    final line = (size * style.height!).ceilToDouble();
    return _top + iconSize + _gap + line * 2 + 4;
  }

  final ArtifactSummary app;
  final VoidCallback onTap;
  final VoidCallback onLongPress;
  final AppIcons icons;

  /// How far the app's build is, while it goes.
  final ({int done, int total})? building;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final b = building;
    return _HomeCell(
      semantics: b == null
          ? app.title
          : '${app.title}, building, ${b.done} of ${b.total} done',
      onTap: onTap,
      onLongPress: onLongPress,
      icon: Stack(
        fit: StackFit.expand,
        children: [
          LiveAppBadge(
            app: app,
            icons: icons,
            size: iconSize,
            radius: iconRadius,
          ),
          if (b != null)
            DecoratedBox(
              decoration: BoxDecoration(
                color: scheme.surface.withValues(alpha: 0.82),
                borderRadius: BorderRadius.circular(iconRadius),
              ),
              child: Center(
                child: _BuildRing(done: b.done, total: b.total),
              ),
            ),
        ],
      ),
      label: _label(app.title, _labelStyle(context)),
    );
  }
}

/// A make going on, as a launcher tile: its request and a ring
/// (indeterminate until its plan has features, then filled to the features
/// done); or "Waiting for connection" (its run waits for the Claude API),
/// "Waiting…" (for a slot), "Interrupted" (the app's last run ended before
/// it), or "Couldn't make it" once it failed.
class MakingTile extends StatelessWidget {
  const MakingTile({
    super.key,
    required this.session,
    required this.onTap,
    this.link,
  });

  final MakeSession session;
  final VoidCallback onTap;

  /// The plans, for the ring; null shows it indeterminate.
  final BuildLink? link;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: session,
    builder: (context, _) {
      final key = session.planKey;
      final b = link;
      if (b == null || key == null) return _tile(context, null);
      return BuildWatch(
        key: ValueKey('make-plan-$key'),
        link: b,
        appId: key,
        builder: (context, plan, _) =>
            _tile(context, plan == null ? null : planCount(plan)),
      );
    },
  );

  Widget _tile(BuildContext context, ({int done, int total})? count) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final failed = session.failure != null && !session.running;
    final offline = session.running && session.waitingForConnection;
    final status = failed
        ? "Couldn't make it"
        : offline
        ? 'Waiting for connection'
        : session.interrupted
        ? 'Interrupted'
        : session.queued
        ? 'Waiting…'
        : 'Making…';
    // Making shows its ring; the others say where they stand in words.
    final said = failed || offline || session.interrupted || session.queued;
    final style = _labelStyle(context);
    return _HomeCell(
      key: const ValueKey('making-tile'),
      semantics: '${session.request}, $status',
      onTap: onTap,
      icon: DecoratedBox(
        decoration: BoxDecoration(
          color: failed ? scheme.errorContainer : scheme.primaryContainer,
          borderRadius: BorderRadius.circular(_AppTile.iconRadius),
        ),
        child: Center(
          child: !said
              ? count == null || count.total == 0
                    ? const _MakingRing()
                    : _BuildRing(done: count.done, total: count.total)
              : Icon(
                  key: offline ? const ValueKey('tile-offline') : null,
                  failed
                      ? Icons.error_outline_rounded
                      : offline
                      ? Icons.cloud_off_rounded
                      : session.interrupted
                      ? Icons.replay_rounded
                      : Icons.hourglass_empty_rounded,
                  color: failed
                      ? scheme.onErrorContainer
                      : scheme.onPrimaryContainer,
                  size: 28,
                ),
        ),
      ),
      label: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          _label(session.request, style, lines: said ? 1 : 2),
          if (said)
            _label(
              status,
              style.copyWith(
                color: failed ? scheme.error : scheme.onSurfaceVariant,
              ),
              lines: 1,
            ),
        ],
      ),
    );
  }
}

/// A make's ring before its plan has features.
class _MakingRing extends StatelessWidget {
  const _MakingRing();

  @override
  Widget build(BuildContext context) => SizedBox.square(
    key: const ValueKey('tile-making'),
    dimension: 36,
    child: Padding(
      padding: const EdgeInsets.all(4),
      child: CircularProgressIndicator(
        strokeWidth: 3,
        strokeCap: StrokeCap.round,
        backgroundColor: Theme.of(context).colorScheme.surfaceContainerHighest,
      ),
    ),
  );
}

/// A tile's building state: a ring filled to the features done, with
/// "3/7" in it.
class _BuildRing extends StatelessWidget {
  const _BuildRing({required this.done, required this.total});

  final int done;
  final int total;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    return SizedBox.square(
      key: const ValueKey('tile-building'),
      dimension: 36,
      child: Stack(
        fit: StackFit.expand,
        children: [
          CircularProgressIndicator(
            value: total == 0 ? null : (done / total).clamp(0.04, 1.0),
            strokeWidth: 3,
            strokeCap: StrokeCap.round,
            backgroundColor: scheme.surfaceContainerHighest,
          ),
          Center(
            child: Text(
              '$done/$total',
              style: theme.textTheme.labelSmall?.copyWith(
                fontWeight: FontWeight.w600,
                fontFeatures: const [FontFeature.tabularFigures()],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _Message extends StatelessWidget {
  const _Message({
    required this.icon,
    required this.text,
    this.hint,
    this.action,
  });

  final IconData icon;
  final String text;
  final String? hint;
  final Widget? action;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    return Padding(
      padding: const EdgeInsets.fromLTRB(32, 0, 32, 96),
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          Icon(icon, size: 48, color: scheme.primary),
          const SizedBox(height: 16),
          Text(
            text,
            textAlign: TextAlign.center,
            style: theme.textTheme.titleLarge,
          ),
          if (hint != null) ...[
            const SizedBox(height: 8),
            Text(
              hint!,
              textAlign: TextAlign.center,
              style: theme.textTheme.bodyLarge?.copyWith(
                color: scheme.onSurfaceVariant,
              ),
            ),
          ],
          if (action != null) ...[const SizedBox(height: 16), action!],
        ],
      ),
    );
  }
}

/// What an app's tile shows in its badge.
sealed class TileFace {
  const TileFace();
}

/// The artifact's own icon, when it is an emoji.
class EmojiFace extends TileFace {
  const EmojiFace(this.emoji);
  final String emoji;

  @override
  bool operator ==(Object other) => other is EmojiFace && other.emoji == emoji;

  @override
  int get hashCode => emoji.hashCode;
}

/// The Material icon for the artifact's icon word.
class IconFace extends TileFace {
  const IconFace(this.icon);
  final IconData icon;

  @override
  bool operator ==(Object other) => other is IconFace && other.icon == icon;

  @override
  int get hashCode => icon.hashCode;
}

/// One or two capital letters from the title's first words.
class MonogramFace extends TileFace {
  const MonogramFace(this.letters);
  final String letters;

  @override
  bool operator ==(Object other) =>
      other is MonogramFace && other.letters == letters;

  @override
  int get hashCode => letters.hashCode;
}

/// The badge of [app]'s tile: its icon when that is an emoji or a word
/// [iconFor] knows, else a monogram of its title's first two words.
TileFace tileFace(ArtifactSummary app) {
  final icon = app.icon?.trim() ?? '';
  if (icon.isNotEmpty && !RegExp(r'^[\x00-\x7F]+$').hasMatch(icon)) {
    return EmojiFace(icon.characters.first);
  }
  final known = iconFor(icon);
  if (known != null) return IconFace(known);
  final words = app.title
      .trim()
      .split(RegExp(r'\s+'))
      .where((w) => w.isNotEmpty);
  if (words.isEmpty) return const MonogramFace('?');
  return MonogramFace(
    words.take(2).map((w) => w.characters.first.toUpperCase()).join(),
  );
}

/// The seed colour of an artifact's tile: a hue taken from a hash of its
/// [id], so each app keeps its colour and neighbours differ.
Color tileSeed(String id) {
  var h = 0x811c9dc5;
  for (final c in id.codeUnits) {
    h = ((h ^ c) * 0x01000193) & 0xffffffff;
  }
  // Mix the last bits through (murmur3's finalizer), so IDs that differ
  // only in their last character still land on distant hues.
  h ^= h >> 16;
  h = (h * 0x85ebca6b) & 0xffffffff;
  h ^= h >> 13;
  h = (h * 0xc2b2ae35) & 0xffffffff;
  h ^= h >> 16;
  return HSLColor.fromAHSL(1, (h % 360).toDouble(), 0.5, 0.5).toColor();
}

final _tileSchemes = <(String, Brightness), ColorScheme>{};

/// The colour scheme of artifact [id]'s tile and inbox rows in [b], seeded
/// by [tileSeed].
ColorScheme tileScheme(String id, Brightness b) => _tileSchemes.putIfAbsent((
  id,
  b,
), () => ColorScheme.fromSeed(seedColor: tileSeed(id), brightness: b));

class _KeyBanner extends StatelessWidget {
  const _KeyBanner({required this.onAdd});

  final VoidCallback? onAdd;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
      child: Material(
        color: scheme.secondaryContainer,
        borderRadius: BorderRadius.circular(20),
        child: Padding(
          padding: const EdgeInsets.fromLTRB(16, 12, 8, 12),
          child: Row(
            children: [
              Icon(Icons.key_rounded, color: scheme.onSecondaryContainer),
              const SizedBox(width: 12),
              Expanded(
                child: Text(
                  'Add your Claude API key to start making apps',
                  style: theme.textTheme.bodyMedium?.copyWith(
                    color: scheme.onSecondaryContainer,
                  ),
                ),
              ),
              if (onAdd != null)
                TextButton(onPressed: onAdd, child: const Text('Add key')),
            ],
          ),
        ),
      ),
    );
  }
}

/// The Material icon for an artifact's icon word (`chart`, `calendar`,
/// `recipe`, …), or null for a word it does not know.
IconData? iconFor(String? word) => switch (word?.trim().toLowerCase()) {
  'chart' || 'graph' || 'stats' || 'analytics' => Icons.insert_chart_rounded,
  'calendar' || 'schedule' || 'date' => Icons.calendar_month_rounded,
  'recipe' || 'food' || 'cooking' || 'meal' => Icons.restaurant_rounded,
  'run' ||
  'running' ||
  'fitness' ||
  'workout' ||
  'exercise' => Icons.directions_run_rounded,
  'list' ||
  'todo' ||
  'checklist' ||
  'tasks' ||
  'task' => Icons.checklist_rounded,
  'note' ||
  'notes' ||
  'journal' ||
  'diary' ||
  'text' => Icons.edit_note_rounded,
  'money' ||
  'budget' ||
  'finance' ||
  'expense' ||
  'expenses' => Icons.savings_rounded,
  'shopping' ||
  'cart' ||
  'groceries' ||
  'grocery' => Icons.shopping_cart_rounded,
  'timer' || 'clock' || 'time' || 'stopwatch' => Icons.timer_rounded,
  'map' || 'location' || 'travel' || 'trip' => Icons.map_rounded,
  'book' || 'books' || 'reading' || 'library' => Icons.menu_book_rounded,
  'music' || 'song' || 'audio' => Icons.music_note_rounded,
  'photo' || 'image' || 'camera' || 'gallery' => Icons.photo_rounded,
  'game' || 'games' || 'puzzle' => Icons.sports_esports_rounded,
  'habit' || 'habits' || 'streak' => Icons.local_fire_department_rounded,
  'water' || 'drink' => Icons.water_drop_rounded,
  'sleep' || 'moon' => Icons.bedtime_rounded,
  'health' || 'medical' || 'heart' => Icons.favorite_rounded,
  'weather' || 'sun' => Icons.wb_sunny_rounded,
  'home' || 'house' => Icons.home_rounded,
  'people' || 'contacts' || 'team' || 'person' => Icons.people_rounded,
  'chat' || 'message' || 'mail' || 'email' => Icons.chat_bubble_rounded,
  'code' || 'dev' || 'terminal' => Icons.code_rounded,
  'calculator' || 'math' => Icons.calculate_rounded,
  'dashboard' || 'home-screen' => Icons.dashboard_rounded,
  'plant' || 'garden' || 'plants' => Icons.local_florist_rounded,
  'pet' || 'dog' || 'cat' => Icons.pets_rounded,
  'car' || 'drive' || 'fuel' => Icons.directions_car_rounded,
  'star' || 'favorite' || 'rating' => Icons.star_rounded,
  'settings' || 'tool' || 'tools' => Icons.build_rounded,
  'app' || 'apps' || 'widget' => Icons.widgets_rounded,
  _ => null,
};
