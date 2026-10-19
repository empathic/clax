import 'dart:async';
import 'dart:typed_data';

import 'package:flutter/material.dart';

import '../agent/timings.dart';
import '../bundle/lineage_store.dart' show LineageEntry;
import '../clax/api.dart';
import '../home/home_host.dart';
import '../home/home_route.dart' show HomeBanner;
import '../home/home_settings.dart';
import '../platform/voice.dart';
import '../agent/plan.dart';
import '../agent/worker.dart';
import 'app_icon.dart';
import 'app_inspector.dart';
import 'build_plan.dart';
import 'bundle_sheets.dart';
import 'inbox_screen.dart';
import 'kit/kit.dart';
import 'layout.dart';
import 'make_service.dart';
import 'making_screen.dart';
import 'new_sheet.dart';
import 'theme.dart';
import 'tile_label.dart';
import 'tokens.g.dart';

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
/// What it shows comes from [host] (`HomeHost`, which follows `gallery`
/// and `inbox` on the app's stream, the makes and the builds), or from a
/// host of its own over [stream] (the app's shared stream, which it never
/// closes; a stream of its own when none is given), [makes] and [build]
/// when none is given. Coming back from the inbox loads the count again.
/// The host's home app is left out of the grid.
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
///
/// With [banner] (the home app failed), a banner above the apps says "Your
/// home screen didn't load", with Undo last change and Reset.
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
    this.host,
    this.banner,
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

  /// The home screen's sources; when null, the launcher keeps a host of
  /// its own over [stream], [makes] (or its own), [build] and [icons],
  /// told [needsKey], whose calls it answers itself.
  final HomeHost? host;

  /// The home app's recovery, shown as a banner above the apps; null
  /// shows none.
  final HomeBanner? banner;

  @override
  State<Launcher> createState() => _LauncherState();
}

class _LauncherState extends State<Launcher> {
  late final MakeService _makes =
      widget.host?.makes ?? widget.makes ?? MakeService(widget.make);

  late final AppIcons _icons =
      widget.host?.icons ?? widget.icons ?? AppIcons(widget.api);

  late final HomeHost _host =
      widget.host ??
      HomeHost(
        api: widget.api,
        stream: widget.stream ?? widget.api.stream(),
        closeStream: widget.stream == null,
        makes: _makes,
        builds: widget.build,
        settings: HomeSettings(),
        actions: HomeFlows(
          contextOf: () => mounted ? context : null,
          api: widget.api,
          openApp: (aid) => widget.open(aid),
          voice: widget.voice,
          showSettings: widget.openSettings,
          onOpenTarget: widget.onOpenTarget,
          onOpenQuestion: widget.onOpenQuestion,
          timings: widget.timings,
          handOver: widget.handOver,
          inspectorPermissions: widget.inspectorPermissions,
          build: widget.build,
          bundles: widget.bundles,
          lineage: widget.lineage,
        ),
        icons: _icons,
        retryDelay: widget.retryDelay,
        needsKey: widget.needsKey,
      );

  /// What the launcher's buttons, tiles and menu open: its host's.
  HomeActions get _actions => _host.actions;

  @override
  void initState() {
    super.initState();
    if (widget.host == null) (_host.actions as HomeFlows).host = _host;
    _host.addListener(_onHost);
  }

  @override
  void didUpdateWidget(Launcher old) {
    super.didUpdateWidget(old);
    if (widget.host == null) _host.needsKey = widget.needsKey;
  }

  void _onHost() {
    if (mounted) setState(() {});
  }

  @override
  void dispose() {
    _host.removeListener(_onHost);
    if (widget.host == null) _host.dispose();
    if (widget.host == null && widget.makes == null) _makes.dispose();
    if (widget.host == null && widget.icons == null) _icons.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final apps = _host.summaries;
    final unread = _host.unread;
    final failed = apps == null && _host.loadError != null;
    final empty = apps != null && apps.isEmpty && _makes.makes.isEmpty;
    return Scaffold(
      // A failed list offers no New app, and an empty one offers it in its
      // invitation.
      floatingActionButton: failed || empty
          ? null
          : FloatingActionButton.extended(
              onPressed: _actions.newApp,
              icon: const Icon(Icons.mic_rounded),
              label: const Text('New app'),
            ),
      body: RefreshIndicator(
        onRefresh: _host.load,
        child: CustomScrollView(
          physics: const AlwaysScrollableScrollPhysics(),
          slivers: [
            SliverAppBar(
              pinned: true,
              title: const Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  ClaxMark(size: 24),
                  SizedBox(width: 10),
                  Text('Clax'),
                ],
              ),
              actions: [
                IconButton(
                  tooltip: 'Inbox',
                  onPressed: _actions.openInbox,
                  icon: ClaxBadge(
                    count: unread,
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
                const SizedBox(width: 8),
              ],
            ),
            if (widget.banner case final b?)
              SliverToBoxAdapter(child: _RecoveryBanner(actions: b)),
            if (_host.needsKey)
              SliverToBoxAdapter(child: _KeyBanner(onAdd: widget.openSettings)),
            if (failed)
              SliverFillRemaining(
                hasScrollBody: true,
                child: ClaxEmpty(
                  key: const ValueKey('launcher-error'),
                  icon: Icons.apps_rounded,
                  title: 'Couldn’t show your apps',
                  // Every app is on this device, from Clax's own service.
                  body: 'Clax’s service didn’t answer.',
                  actions: [
                    ClaxButton(
                      label: 'Try again',
                      kind: ClaxButtonKind.primary,
                      icon: Icons.refresh_rounded,
                      onPressed: () => unawaited(_host.load()),
                    ),
                  ],
                ),
              )
            else if (apps == null)
              const SliverFillRemaining(
                hasScrollBody: false,
                child: SizedBox.shrink(),
              )
            else if (empty)
              SliverFillRemaining(
                hasScrollBody: true,
                child: ClaxEmpty(
                  key: const ValueKey('launcher-empty'),
                  art: const ClaxMark(size: 64),
                  title: 'Say what you need, and Clax makes it.',
                  body: 'Like “a tracker for my runs, with a weekly total.”',
                  actions: [
                    ClaxButton(
                      key: const ValueKey('launcher-empty-new'),
                      label: 'New app',
                      kind: ClaxButtonKind.primary,
                      isDefault: true,
                      icon: Icons.mic_rounded,
                      onPressed: _actions.newApp,
                    ),
                  ],
                ),
              )
            else
              // Centred, at most ClaxLayout.grid wide, inside 16 dp gutters.
              SliverLayoutBuilder(
                builder: (context, c) {
                  final side = ((c.crossAxisExtent - ClaxLayout.grid) / 2)
                      .clamp(16.0, double.infinity);
                  return SliverPadding(
                    padding: EdgeInsets.fromLTRB(side, 8, side, 112),
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
                            onTap: () => _actions.openMake(_host.makeIdOf(m)),
                          );
                        }
                        i -= makes.length;
                        return _AppTile(
                          key: ValueKey('build-${apps[i].id}'),
                          app: apps[i],
                          icons: _icons,
                          building: _host.buildOf(apps[i].id),
                          onTap: () => widget.open(apps[i].id),
                          onLongPress: (at) => unawaited(
                            _actions.menu(apps[i].id, at, const []),
                          ),
                        );
                      },
                    ),
                  );
                },
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

  /// Called with the cell's rect in global coordinates, where a menu
  /// opens.
  final ValueChanged<Rect>? onLongPress;

  @override
  Widget build(BuildContext context) => Semantics(
    button: true,
    label: semantics,
    excludeSemantics: true,
    child: InkWell(
      onTap: onTap,
      onLongPress: switch (onLongPress) {
        final f? => () {
          final box = context.findRenderObject()! as RenderBox;
          f(box.localToGlobal(Offset.zero) & box.size);
        },
        null => null,
      },
      borderRadius: BorderRadius.circular(20),
      child: Padding(
        padding: const EdgeInsets.fromLTRB(4, _AppTile._top, 4, 4),
        child: Column(
          children: [
            SizedBox.square(dimension: _AppTile.iconSize(context), child: icon),
            const SizedBox(height: _AppTile._gap),
            label,
          ],
        ),
      ),
    ),
  );
}

class _AppTile extends StatelessWidget {
  const _AppTile({
    super.key,
    required this.app,
    required this.onTap,
    required this.onLongPress,
    required this.icons,
    this.building,
  });

  /// The side of a cell's icon: 60 on a phone, 64 from `ClaxWidth.medium`.
  static double iconSize(BuildContext context) =>
      ClaxWidth.of(context) >= ClaxWidth.medium ? 64 : 60;

  /// The corner radius of a cell's icon: a quarter of its side.
  static double iconRadius(BuildContext context) => iconSize(context) / 4;

  static const _top = 8.0;
  static const _gap = 8.0;

  /// The widest a cell may be: four to a row on a phone, more on wider
  /// screens.
  static const maxWidth = 112.0;

  /// A cell's height: the icon, a gap and two lines of the label at the
  /// current text scale, so a wrapped title always fits.
  static double extent(BuildContext context) {
    final style = TileLabel.styleOf(context);
    final size = MediaQuery.textScalerOf(context).scale(style.fontSize!);
    final line = (size * style.height!).ceilToDouble();
    return _top + iconSize(context) + _gap + line * 2 + 4;
  }

  final ArtifactSummary app;
  final VoidCallback onTap;
  final ValueChanged<Rect> onLongPress;
  final AppIcons icons;

  /// How far the app's build is, while it goes.
  final ({int done, int total})? building;

  @override
  Widget build(BuildContext context) {
    final b = building;
    final badge = LiveAppBadge(
      app: app,
      icons: icons,
      size: iconSize(context),
      radius: iconRadius(context),
    );
    return _HomeCell(
      semantics: b == null
          ? app.title
          : '${app.title}, building, ${b.done} of ${b.total} done',
      onTap: onTap,
      onLongPress: onLongPress,
      icon: b == null
          ? badge
          : _Building(
              key: const ValueKey('tile-building'),
              radius: iconRadius(context),
              count: '${b.done}/${b.total}',
              child: badge,
            ),
      label: TileLabel(app.title),
    );
  }
}

/// An icon whose build goes: [child] at full strength in a 3 dp `agent`
/// ring, with [count] ("2/5") in a [ClaxBadge] at its top end corner.
class _Building extends StatelessWidget {
  const _Building({
    super.key,
    required this.radius,
    required this.count,
    required this.child,
  });

  final double radius;
  final String count;
  final Widget child;

  static const _gap = 2.0;
  static const _ring = 3.0;

  @override
  Widget build(BuildContext context) {
    const out = _gap + _ring;
    return Stack(
      fit: StackFit.expand,
      clipBehavior: Clip.none,
      children: [
        child,
        Positioned(
          left: -out,
          top: -out,
          right: -out,
          bottom: -out,
          child: IgnorePointer(
            child: DecoratedBox(
              decoration: BoxDecoration(
                border: Border.all(
                  color: claxPalette(context).agent,
                  width: _ring,
                ),
                borderRadius: BorderRadius.circular(radius + out),
              ),
            ),
          ),
        ),
        PositionedDirectional(
          top: -7,
          end: -9,
          child: ClaxBadge.label(count),
        ),
      ],
    );
  }
}

/// A make going on, as a launcher tile: its request on one line and how
/// it stands on the second, in that state's colour: "Making…" (its face
/// the breathing [ClaxMark] on `accent-tint`, in a ring filled to the
/// features done once its plan has features, with "3/7" in a badge),
/// "Offline" (its run waits for the Claude API's connection, read out as
/// "Waiting for connection"), "Waiting…" (for a slot), "Interrupted" (the
/// app's last run ended before it), or "Couldn't make it", in `danger`,
/// once it failed.
class MakingTile extends StatelessWidget {
  const MakingTile({
    super.key,
    required this.session,
    required this.onTap,
    this.link,
  });

  final MakeSession session;
  final VoidCallback onTap;

  /// The plans, for the ring; null shows the mark alone.
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
    final p = claxPalette(context);
    final failed = session.failure != null && !session.running;
    final stopped = session.stopped != null && !session.running;
    final offline = session.running && session.waitingForConnection;
    final status = failed
        ? 'Couldn’t make it'
        : stopped
        ? 'Made a start'
        : offline
        ? 'Offline'
        : session.interrupted
        ? 'Interrupted'
        : session.queued
        ? 'Waiting…'
        : 'Making…';
    // Making shows the breathing mark; the others say where they stand
    // with a glyph.
    final said =
        failed || stopped || offline || session.interrupted || session.queued;
    final counted = !said && count != null && count.total > 0;
    final face = DecoratedBox(
      decoration: BoxDecoration(
        color: failed ? p.dangerTint : p.accentTint,
        borderRadius: BorderRadius.circular(_AppTile.iconRadius(context)),
      ),
      child: Center(
        child: !said
            ? counted
                  ? _MakeRing(done: count.done, total: count.total)
                  : const ClaxMark(
                      key: ValueKey('tile-making'),
                      size: 28,
                      state: ClaxMarkState.working,
                    )
            : Icon(
                key: offline ? const ValueKey('tile-offline') : null,
                failed
                    ? Icons.error_outline_rounded
                    : stopped
                    ? Icons.pause_circle_outline_rounded
                    : offline
                    ? Icons.cloud_off_rounded
                    : session.interrupted
                    ? Icons.replay_rounded
                    : Icons.hourglass_empty_rounded,
                color: failed ? p.danger : p.agentInk,
                size: 28,
              ),
      ),
    );
    return _HomeCell(
      key: const ValueKey('making-tile'),
      semantics:
          '${session.request}, ${offline ? 'Waiting for connection' : status}',
      onTap: onTap,
      icon: counted
          ? Stack(
              fit: StackFit.expand,
              clipBehavior: Clip.none,
              children: [
                face,
                PositionedDirectional(
                  top: -7,
                  end: -9,
                  child: ClaxBadge.label('${count.done}/${count.total}'),
                ),
              ],
            )
          : face,
      label: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          TileLabel(session.request, maxLines: 1),
          Text(
            status,
            key: const ValueKey('making-status'),
            maxLines: 1,
            softWrap: false,
            overflow: TextOverflow.ellipsis,
            textAlign: TextAlign.center,
            style: TileLabel.styleOf(context).copyWith(
              color: failed
                  ? p.danger
                  : said
                  ? p.muted
                  : p.agentInk,
            ),
          ),
        ],
      ),
    );
  }
}

/// A make's progress: the breathing mark in a ring of `agent` on `border`
/// filled to the features done.
class _MakeRing extends StatelessWidget {
  const _MakeRing({required this.done, required this.total});

  final int done;
  final int total;

  @override
  Widget build(BuildContext context) {
    final p = claxPalette(context);
    return SizedBox.square(
      key: const ValueKey('tile-making'),
      dimension: 36,
      child: Stack(
        fit: StackFit.expand,
        children: [
          CircularProgressIndicator(
            value: (done / total).clamp(0.04, 1.0),
            strokeWidth: 3,
            strokeCap: StrokeCap.round,
            color: p.agent,
            backgroundColor: p.border,
          ),
          const Center(
            child: ClaxMark(size: 20, state: ClaxMarkState.working),
          ),
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
Color tileSeed(String id) =>
    HSLColor.fromAHSL(1, tileHue(id).toDouble(), 0.5, 0.5).toColor();

/// The hue (0 to 359) of artifact [id]'s tile ([tileSeed]).
int tileHue(String id) {
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
  return h % 360;
}

final _tileSchemes = <(String, Brightness), ColorScheme>{};

/// The colour scheme of artifact [id]'s tile and inbox rows in [b], seeded
/// by [tileSeed].
ColorScheme tileScheme(String id, Brightness b) => _tileSchemes.putIfAbsent((
  id,
  b,
), () => ColorScheme.fromSeed(seedColor: tileSeed(id), brightness: b));

/// "Your home screen didn't load", with Undo last change and Reset home
/// screen. Under the title it says that both change only the home screen
/// (the apps and the arrangement stay), or the sentence of an action that
/// failed; both buttons wait while one runs.
class _RecoveryBanner extends StatefulWidget {
  const _RecoveryBanner({required this.actions});

  final HomeBanner actions;

  @override
  State<_RecoveryBanner> createState() => _RecoveryBannerState();
}

class _RecoveryBannerState extends State<_RecoveryBanner> {
  var _busy = false;
  String? _error;

  Future<void> _run(Future<String?> Function() action) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    final error = await action();
    if (!mounted) return;
    setState(() {
      _busy = false;
      _error = error;
    });
  }

  @override
  Widget build(BuildContext context) => Padding(
    key: const ValueKey('home-recovery-banner'),
    padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
    child: ClaxBanner(
      icon: Icons.home_rounded,
      iconColor: claxPalette(context).danger,
      title: 'Your home screen didn’t load',
      body:
          _error ??
          'Clax is showing a simpler one for now. Undo and Reset change only '
              'the home screen: your apps, their data, stars and folders '
              'stay.',
      actions: [
        if (_busy)
          const SizedBox.square(
            dimension: 16,
            child: CircularProgressIndicator(strokeWidth: 2),
          ),
        ClaxButton(
          label: 'Undo last change',
          onPressed: _busy ? null : () => _run(widget.actions.undo),
        ),
        ClaxButton(
          label: 'Reset home screen',
          onPressed: _busy ? null : () => _run(widget.actions.reset),
        ),
      ],
    ),
  );
}

class _KeyBanner extends StatelessWidget {
  const _KeyBanner({required this.onAdd});

  final VoidCallback? onAdd;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
    child: ClaxBanner(
      icon: Icons.key_rounded,
      title: 'Add your Claude API key to start making apps',
      actions: [
        if (onAdd != null) ClaxButton(label: 'Add key', onPressed: onAdd),
      ],
    ),
  );
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

/// The launcher's own answers to its host's calls, when it keeps the host.
/// The screens and sheets the home screen opens, over the navigator of
/// [context]: the [HomeActions] of a [host] (a `HomeHost`), for the
/// built-in home screen ([Launcher]) and the home app alike.
///
/// New app shows the New sheet; a submitted request starts a make on the
/// host's makes and shows it in the build view; the artifact ID it returns
/// goes to [handOver] (which puts the app's route in the build view's
/// place), else to [open] once the build view has closed. A request the
/// person has not made yet (the sheet closed, Cancel, or Edit after a
/// failure) is kept for the next New. A make's tile shows its build view
/// again, caught up, or offers an interrupted one again ("This wasn’t
/// finished.").
///
/// The menu of an app: the home app's items, above a divider, then Open;
/// Inspect, which pushes the [AppInspector] with
/// [inspectorPermissions] (a version opened from its history going to
/// [onOpenTarget]); with [bundles], Share app; Resume build for a build
/// that stopped with features to build. The inbox pushes [InboxScreen] on
/// the host's stream; an item opened there goes to [onOpenQuestion] (a
/// question, when given), else to [onOpenTarget], else to [open]; coming
/// back loads the unread count again.
class HomeFlows implements HomeActions {
  HomeFlows({
    required this.contextOf,
    required this.api,
    required this.openApp,
    required this.voice,
    this.showSettings,
    this.onOpenTarget,
    this.onOpenQuestion,
    this.timings,
    this.handOver,
    this.inspectorPermissions = const [],
    this.build,
    this.bundles,
    this.lineage,
  });

  /// The context the screens are pushed from; null once there is none.
  final BuildContext? Function() contextOf;
  final ClaxApi api;

  /// Opens app [aid], as a tile's tap does.
  final void Function(String aid) openApp;
  final VoiceInput voice;

  /// Shows Settings; null shows none.
  final VoidCallback? showSettings;
  final void Function(
    String aid, {
    String? thread,
    int? version,
    String? comment,
  })?
  onOpenTarget;
  final void Function(String qid)? onOpenQuestion;
  final Timings? timings;
  final void Function(BuildContext context, String aid, Uint8List? lastFrame)?
  handOver;
  final List<PermissionSource> inspectorPermissions;
  final BuildLink? build;
  final BundleFlows? bundles;
  final Future<LineageEntry?> Function(String aid)? lineage;

  /// The host whose makes, apps and stream these flows act on; set once,
  /// before the first action.
  late final HomeHost host;

  String _draft = '';

  bool get mounted => contextOf()?.mounted ?? false;
  BuildContext get context => contextOf()!;

  Future<void> _openInbox() async {
    await Navigator.of(context).push<void>(
      MaterialPageRoute(
        builder: (_) => InboxScreen(
          api: api,
          onOpen: _openItem,
          stream: host.stream,
          icons: host.icons,
        ),
      ),
    );
    if (mounted) await host.loadUnread();
  }

  void _openItem(InboxItem item) {
    final question = item.question;
    if (item.kind == 'question' && question != null && onOpenQuestion != null) {
      onOpenQuestion!(question.id);
      return;
    }
    final aid = item.artifactId;
    if (aid == null) return;
    final target = onOpenTarget;
    if (target == null) {
      openApp(aid);
      return;
    }
    target(
      aid,
      thread: item.kind == 'reply' ? item.thread?.id : null,
      version: item.kind == 'version' ? item.versionN : null,
      comment: item.kind == 'reply' ? item.replyCommentId : null,
    );
  }

  /// Shows make [s]'s build view, or offers an interrupted one again.
  Future<void> _openMake(MakeSession s) =>
      s.interrupted ? _offerAgain(s) : _showMake(s);

  Future<void> _newApp() async {
    final request = await showNewSheet(
      context,
      voice,
      _draft,
      onChanged: (t) => _draft = t,
    );
    if (request == null || !mounted) return;
    _draft = request;
    // The build view starts it, once it is up.
    await _showMake(host.makes.add(request));
  }

  /// Says interrupted make [s] wasn't finished, and offers **Make it
  /// again** (which shows it) or **Discard** (which forgets it); closing
  /// the question (Escape, a tap outside, Back) leaves it as it is.
  Future<void> _offerAgain(MakeSession s) async {
    final choice = await ClaxDialog.show(
      context,
      title: 'This wasn’t finished.',
      body: '“${s.request}”',
      actions: const [
        ClaxDialogAction('Discard', kind: ClaxButtonKind.destructive),
        ClaxDialogAction('Make it again', kind: ClaxButtonKind.primary),
      ],
      defaultIndex: 1,
    );
    if (!mounted || choice == null) return;
    final again = choice == 1;
    if (!again) {
      host.makes.dismiss(s);
      return;
    }
    host.makes.resume(s);
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
          openSettings: showSettings,
          timings: timings,
          onMade: handOver,
          build: build,
        ),
      ),
    );
    if (!mounted) return;
    switch (result) {
      case Made(:final aid):
        if (_draft == s.request) _draft = '';
        if (handOver == null) openApp(aid);
      case MakingLeft():
        if (_draft == s.request) _draft = '';
      case EditRequest():
        _draft = s.request;
        await _newApp();
      case null:
        _draft = s.request;
    }
  }

  /// Whether [aid]'s build stopped (stopped, capped or failed) with
  /// features still to build, so the menu offers Resume build.
  Future<bool> _resumable(String aid) async {
    final b = build;
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

  /// Shows [app]'s menu anchored at [anchor] (global coordinates): [items]
  /// first, then Clax's own. Resolves with the picked item's ID as the menu
  /// closes, or null for one of Clax's own (whose action then runs) or a
  /// dismissal.
  Future<String?> _menu(
    ArtifactSummary app,
    Rect anchor,
    List<HomeMenuItem> items,
  ) async {
    final resume = await _resumable(app.id);
    final at = contextOf();
    if (at == null || !at.mounted) return null;
    const own = 'home-item-';
    final choice = await ClaxMenu.show(
      at,
      anchor: anchor,
      items: [
        for (final i in items) ClaxMenuItem('$own${i.id}', i.label),
        if (items.isNotEmpty) const ClaxMenuItem.separator(),
        const ClaxMenuItem(
          'open',
          'Open',
          icon: Icons.open_in_new_rounded,
          symbol: 'arrow.up.forward.app',
        ),
        const ClaxMenuItem(
          'inspect',
          'Inspect',
          icon: Icons.manage_search_rounded,
          symbol: 'magnifyingglass',
        ),
        if (bundles case final b?)
          ClaxMenuItem(
            'share-app',
            b.shareLabel,
            icon: b.toFile ? Icons.save_alt_rounded : Icons.ios_share_rounded,
            symbol: 'square.and.arrow.up',
          ),
        if (resume)
          const ClaxMenuItem(
            'resume-build',
            'Resume build',
            icon: Icons.play_arrow_rounded,
            symbol: 'play',
          ),
      ],
    );
    if (choice == null) return null;
    if (choice.startsWith(own)) return choice.substring(own.length);
    unawaited(_act(app, choice, anchor));
    return null;
  }

  /// Runs Clax's own menu item [choice] on [app], whose menu opened at
  /// [anchor].
  Future<void> _act(ArtifactSummary app, String choice, Rect anchor) async {
    final c = contextOf();
    if (c == null || !c.mounted) return;
    switch (choice) {
      case 'open':
        openApp(app.id);
      case 'resume-build':
        await build?.control.resumeBuild(app.id);
      case 'share-app':
        // The share sheet points at the tile, where the menu opened; a menu
        // with no anchor leaves it the screen's centre.
        await bundles?.share(
          c,
          app.id,
          svg: host.icons?.svgOf(app.id),
          origin: anchor.isEmpty ? null : anchor,
        );
      case 'inspect':
        final target = onOpenTarget;
        await Navigator.of(c).push<void>(
          MaterialPageRoute(
            builder: (_) => AppInspector(
              api: api,
              aid: app.id,
              permissions: inspectorPermissions,
              bundles: bundles,
              lineage: lineage,
              plan: switch (build) {
                final b? => (context, aid) => BuildWatch(
                  link: b,
                  appId: aid,
                  builder: (context, plan, state) => plan == null
                      ? const ClaxEmpty(
                          icon: Icons.checklist_rounded,
                          title: 'No plan',
                          body:
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
  void open(String id) => openApp(id);

  @override
  void openMake(String id) {
    if (!mounted) return;
    if (host.sessionOf(id) case final m?) unawaited(_openMake(m));
  }

  @override
  void newApp() {
    if (mounted) unawaited(_newApp());
  }

  @override
  Future<String?> menu(String id, Rect anchor, List<HomeMenuItem> items) async {
    if (!mounted) return null;
    for (final a in host.summaries ?? const <ArtifactSummary>[]) {
      if (a.id == id) return _menu(a, anchor, items);
    }
    return null;
  }

  @override
  void openInbox() {
    if (mounted) unawaited(_openInbox());
  }

  @override
  void openSettings() => showSettings?.call();

  @override
  void importApp() {
    if (bundles case final b? when mounted) unawaited(b.importPicked(context));
  }
}
