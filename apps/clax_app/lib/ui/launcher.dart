import 'dart:async';

import 'package:flutter/material.dart';

import '../clax/api.dart';
import '../platform/voice.dart';
import 'making_screen.dart';
import 'new_sheet.dart';

/// The home screen: the owner's apps, newest first, live from the daemon's
/// events, and the New app button.
///
/// New opens the New sheet (listening at once when dictation is available);
/// a submitted request goes to [make] behind the making screen, and the
/// artifact ID it returns goes to [open]. A request the person has not
/// made yet (the sheet closed, Cancel, or Edit after a failure) is kept for
/// the next New.
///
/// The list is loaded at start and again whenever the event stream says
/// events may have been missed: `resync`, or a `ready` that did not resume
/// (a new daemon, or a reconnect it could not resume).
class Launcher extends StatefulWidget {
  const Launcher({
    super.key,
    required this.api,
    required this.open,
    required this.make,
    required this.voice,
    this.openSettings,
    this.needsKey = false,
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

  @override
  State<Launcher> createState() => _LauncherState();
}

class _LauncherState extends State<Launcher> {
  List<ArtifactSummary>? _apps;
  Object? _loadError;
  StreamSubscription<ClaxEvent>? _events;
  String _draft = '';

  /// Bumped by each load, so only the newest load's answer is kept.
  int _loads = 0;

  @override
  void initState() {
    super.initState();
    _events = widget.api
        .events(types: const ['version', 'artifact_deleted'])
        .listen(_onEvent, onError: (Object _) {});
    _load();
  }

  @override
  void dispose() {
    _events?.cancel();
    super.dispose();
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

  Future<void> _onEvent(ClaxEvent e) async {
    if (e.type == 'resync' ||
        (e.type == 'ready' && e.data['resumed'] != true)) {
      await _load();
      return;
    }
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
    final result = await Navigator.of(context).push<MakingResult>(
      MaterialPageRoute(
        fullscreenDialog: true,
        builder: (_) => MakingScreen(
          request: request,
          make: widget.make,
          openSettings: widget.openSettings,
        ),
      ),
    );
    if (!mounted) return;
    switch (result) {
      case Made(:final aid):
        _draft = '';
        widget.open(aid);
      case EditRequest():
        await _newApp();
      case null:
        break;
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
                if (widget.openSettings != null)
                  IconButton(
                    tooltip: 'Settings',
                    onPressed: widget.openSettings,
                    icon: const Icon(Icons.settings_outlined),
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
            else if (apps.isEmpty)
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
                    maxCrossAxisExtent: 176,
                    mainAxisSpacing: 12,
                    crossAxisSpacing: 12,
                    mainAxisExtent: _AppTile.extent(context),
                  ),
                  itemCount: apps.length,
                  itemBuilder: (context, i) => _AppTile(
                    app: apps[i],
                    onTap: () => widget.open(apps[i].id),
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }
}

class _AppTile extends StatelessWidget {
  const _AppTile({required this.app, required this.onTap});

  static const _padding = 16.0;
  static const _badge = 52.0;
  static const _gap = 12.0;

  /// The tile's height: padding, the badge, a gap and two lines of the
  /// title at the current text scale, so a wrapped title always fits.
  static double extent(BuildContext context) {
    final style = Theme.of(context).textTheme.titleMedium!;
    final size = MediaQuery.textScalerOf(context).scale(style.fontSize!);
    final line = (size * (style.height ?? 1.2)).ceilToDouble();
    return _padding * 2 + _badge + _gap + line * 2;
  }

  final ArtifactSummary app;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final tile = _tileScheme(app.id, theme.brightness);
    final (bg, fg) = (tile.primaryContainer, tile.onPrimaryContainer);
    return Semantics(
      button: true,
      label: app.title,
      excludeSemantics: true,
      child: Material(
        color: scheme.surfaceContainerLow,
        borderRadius: BorderRadius.circular(24),
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: onTap,
          child: Padding(
            padding: const EdgeInsets.all(_padding),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Container(
                  width: _badge,
                  height: _badge,
                  decoration: BoxDecoration(
                    color: bg,
                    borderRadius: BorderRadius.circular(16),
                  ),
                  alignment: Alignment.center,
                  child: switch (tileFace(app)) {
                    EmojiFace(:final emoji) => Text(
                      emoji,
                      style: const TextStyle(fontSize: 28, height: 1),
                    ),
                    IconFace(:final icon) => Icon(icon, color: fg, size: 28),
                    MonogramFace(:final letters) => Text(
                      letters,
                      style: theme.textTheme.titleLarge?.copyWith(
                        color: fg,
                        fontWeight: FontWeight.w600,
                        height: 1,
                      ),
                    ),
                  },
                ),
                const Spacer(),
                Text(
                  app.title,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: theme.textTheme.titleMedium,
                ),
              ],
            ),
          ),
        ),
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

ColorScheme _tileScheme(String id, Brightness b) => _tileSchemes.putIfAbsent((
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
