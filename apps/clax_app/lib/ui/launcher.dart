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
class Launcher extends StatefulWidget {
  const Launcher({
    super.key,
    required this.api,
    required this.open,
    required this.make,
    required this.voice,
    this.openSettings,
  });

  final ClaxApi api;
  final void Function(String aid) open;
  final Future<String> Function(String request) make;
  final VoiceInput voice;

  /// Shows Settings; without it the launcher shows no settings button.
  final VoidCallback? openSettings;

  @override
  State<Launcher> createState() => _LauncherState();
}

class _LauncherState extends State<Launcher> {
  List<ArtifactSummary>? _apps;
  Object? _loadError;
  StreamSubscription<ClaxEvent>? _events;
  String _draft = '';

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
    try {
      final apps = await widget.api.listArtifacts();
      if (!mounted) return;
      setState(() {
        _apps = _newestFirst(apps);
        _loadError = null;
      });
    } catch (e) {
      if (mounted) setState(() => _loadError = e);
    }
  }

  static List<ArtifactSummary> _newestFirst(List<ArtifactSummary> apps) =>
      List.of(apps)..sort((a, b) => b.updatedAt.compareTo(a.updatedAt));

  Future<void> _onEvent(ClaxEvent e) async {
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
                  gridDelegate: const SliverGridDelegateWithMaxCrossAxisExtent(
                    maxCrossAxisExtent: 176,
                    mainAxisSpacing: 12,
                    crossAxisSpacing: 12,
                    childAspectRatio: 0.92,
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

  final ArtifactSummary app;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final (bg, fg) = switch (app.id.codeUnits.fold(0, (a, b) => a + b) % 3) {
      0 => (scheme.primaryContainer, scheme.onPrimaryContainer),
      1 => (scheme.secondaryContainer, scheme.onSecondaryContainer),
      _ => (scheme.tertiaryContainer, scheme.onTertiaryContainer),
    };
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
            padding: const EdgeInsets.all(16),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Container(
                  width: 52,
                  height: 52,
                  decoration: BoxDecoration(
                    color: bg,
                    borderRadius: BorderRadius.circular(16),
                  ),
                  child: Icon(iconFor(app.icon), color: fg, size: 28),
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

/// The Material icon for an artifact's icon word (`chart`, `calendar`,
/// `recipe`, …); a generic app icon for a word it does not know.
IconData iconFor(String? word) => switch (word?.trim().toLowerCase()) {
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
  _ => Icons.widgets_rounded,
};
