/// The build's UI: an app's plan as a live checklist the person can edit
/// while it builds, the panel around it (what the build is doing, Stop and
/// Resume), and the plumbing the build view, the Working chip, the launcher
/// and the inspector share.
library;

import 'dart:async';

import 'package:flutter/material.dart';

import '../agent/plan.dart';
import '../agent/progress.dart';
import '../agent/worker.dart';

/// What the build UI needs of the worker's builds ([FeedbackWorker]).
abstract interface class BuildControl {
  /// Each step of every build.
  Stream<BuildState> get builds;

  /// The latest step of [appId]'s build, or null.
  BuildState? buildOf(String appId);

  Future<void> stopBuild(String appId);
  Future<void> resumeBuild(String appId);
}

/// [BuildControl] over a [FeedbackWorker].
class WorkerBuildControl implements BuildControl {
  WorkerBuildControl(this.worker);

  final FeedbackWorker worker;

  @override
  Stream<BuildState> get builds => worker.builds;

  @override
  BuildState? buildOf(String appId) => worker.buildOf(appId);

  @override
  Future<void> stopBuild(String appId) => worker.stopBuild(appId);

  @override
  Future<void> resumeBuild(String appId) => worker.resumeBuild(appId);
}

/// The plans and the builds, as the build view, the Working chip, the
/// launcher and the inspector see them.
class BuildLink {
  const BuildLink({required this.plans, required this.control});

  final PlanStore plans;
  final BuildControl control;

  /// Adds [text] (a comment) as a feature at the end of app [aid]'s plan
  /// and takes its build up again, so the next continue run builds it.
  Future<PlanFeature> addToPlan(String aid, String text) async {
    final t = text.trim();
    final title = featureTitle(t);
    final f = await plans.addFeature(
      aid,
      title: title,
      detail: title == t ? '' : t,
    );
    await control.resumeBuild(aid);
    return f;
  }
}

/// The longest feature title [featureTitle] makes.
const featureTitleMax = 48;

/// A feature title from a comment: its first line, cut at the last word
/// boundary within [featureTitleMax] characters with "…".
String featureTitle(String text) {
  final line = text.trim().split('\n').first.trim();
  if (line.length <= featureTitleMax) return line;
  var cut = line.lastIndexOf(' ', featureTitleMax - 1);
  if (cut <= 0) cut = featureTitleMax - 1;
  return '${line.substring(0, cut).trimRight()}…';
}

/// How far [plan] is: the features done, of those not dropped.
({int done, int total}) planCount(Plan plan) {
  var done = 0;
  var total = 0;
  for (final f in plan.features) {
    if (f.status == FeatureStatus.dropped) continue;
    total++;
    if (f.status == FeatureStatus.done) done++;
  }
  return (done: done, total: total);
}

/// Whether [s] is a build that is going: queued or running.
bool buildGoing(BuildState? s) =>
    s != null &&
    (s.phase == BuildPhase.queued || s.phase == BuildPhase.running);

/// What a progress event says the run is doing: the agent's narration or
/// the label of the tool it started; null for the others.
String? buildStep(AgentProgress? p) => switch (p) {
  Narration(:final text) => text,
  ToolStarted(:final label) => label,
  _ => null,
};

/// The feature [s] builds, looked up in [plan]; else the plan's doing one.
PlanFeature? buildFeature(Plan? plan, BuildState? s) {
  if (plan == null) return null;
  if (s?.featureId case final id?) {
    if (plan.feature(id) case final f?) return f;
  }
  return plan.doing;
}

/// Rebuilds [builder] with app [appId]'s plan and its build's latest step,
/// as each changes.
class BuildWatch extends StatefulWidget {
  const BuildWatch({
    super.key,
    required this.link,
    required this.appId,
    required this.builder,
  });

  final BuildLink link;
  final String appId;
  final Widget Function(BuildContext context, Plan? plan, BuildState? state)
  builder;

  @override
  State<BuildWatch> createState() => _BuildWatchState();
}

class _BuildWatchState extends State<BuildWatch> {
  Plan? _plan;
  BuildState? _state;
  final _subs = <StreamSubscription<Object?>>[];

  @override
  void initState() {
    super.initState();
    _watch();
  }

  @override
  void didUpdateWidget(BuildWatch old) {
    super.didUpdateWidget(old);
    if (old.appId != widget.appId || old.link != widget.link) {
      _unwatch();
      _plan = null;
      _watch();
    }
  }

  @override
  void dispose() {
    _unwatch();
    super.dispose();
  }

  void _watch() {
    final aid = widget.appId;
    _state = widget.link.control.buildOf(aid);
    _subs
      ..add(
        widget.link.plans.watch(aid).listen((p) {
          if (mounted) setState(() => _plan = p);
        }),
      )
      ..add(
        widget.link.control.builds.where((s) => s.appId == aid).listen((s) {
          if (mounted) setState(() => _state = s);
        }),
      );
  }

  void _unwatch() {
    for (final s in _subs) {
      unawaited(s.cancel());
    }
    _subs.clear();
  }

  @override
  Widget build(BuildContext context) => widget.builder(context, _plan, _state);
}

/// What the build is doing, in a line: "Building Export", "Stopped", …
String buildHeadline(Plan? plan, BuildState? s) {
  if (plan == null) return 'Planning the features…';
  final feature = buildFeature(plan, s);
  return switch (s?.phase) {
    BuildPhase.running when feature != null => 'Building ${feature.title}',
    BuildPhase.queued when plan.todo.isNotEmpty =>
      'Up next: ${plan.todo.first.title}',
    BuildPhase.stopped => 'Build stopped',
    BuildPhase.failed => 'Build stopped: ${s?.reason ?? 'it failed'}',
    BuildPhase.capped => 'Build paused at its spending cap',
    BuildPhase.done => 'Every feature is built',
    _ when plan.stopped => 'Build stopped',
    _ when feature != null => 'Building ${feature.title}',
    _ when plan.features.isEmpty => 'Planning the features…',
    _ when plan.todo.isEmpty => 'Every feature is built',
    _ => 'Planned ${planCount(plan).total} features',
  };
}

/// The plan panel: a header (a handle, what the build does now, how far it
/// is, Stop or Resume) over [PlanChecklist]. In a draggable sheet, pass its
/// [controller] so a drag on the header moves the sheet.
///
/// - [onStop] and [onResume], when given, show Stop while the build goes
///   and Resume once it stopped or failed.
/// - [editable]: the person can add, edit, drop and reorder features of
///   the plan kept under [planKey].
class BuildPlanPanel extends StatelessWidget {
  const BuildPlanPanel({
    super.key,
    required this.plan,
    required this.state,
    this.plans,
    this.planKey,
    this.controller,
    this.onStop,
    this.onResume,
    this.handle = true,
  });

  final Plan? plan;
  final BuildState? state;

  /// Where edits go; null for a read-only checklist.
  final PlanStore? plans;
  final String? planKey;
  final ScrollController? controller;
  final VoidCallback? onStop;
  final VoidCallback? onResume;

  /// Whether to draw a sheet's drag handle.
  final bool handle;

  @override
  Widget build(BuildContext context) => PlanChecklist(
    plan: plan,
    state: state,
    plans: plans,
    planKey: planKey,
    controller: controller,
    header: _header(context),
  );

  Widget _header(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final p = plan;
    final s = state;
    final count = p == null ? null : planCount(p);
    final going = buildGoing(s) || (s == null && p != null && !p.stopped);
    final stopped =
        s?.phase == BuildPhase.stopped ||
        s?.phase == BuildPhase.failed ||
        (s == null && (p?.stopped ?? false));
    final Widget? action = switch ((going, stopped)) {
      (true, _) when onStop != null => TextButton.icon(
        key: const ValueKey('build-stop'),
        onPressed: onStop,
        icon: const Icon(Icons.stop_rounded, size: 18),
        label: const Text('Stop'),
      ),
      (_, true) when onResume != null => FilledButton.tonalIcon(
        key: const ValueKey('build-resume'),
        onPressed: onResume,
        icon: const Icon(Icons.play_arrow_rounded, size: 18),
        label: const Text('Resume'),
      ),
      _ => null,
    };
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 0, 12, 4),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (handle)
            Center(
              child: Container(
                width: 32,
                height: 4,
                margin: const EdgeInsets.symmetric(vertical: 10),
                decoration: BoxDecoration(
                  color: scheme.onSurfaceVariant.withValues(alpha: 0.4),
                  borderRadius: BorderRadius.circular(2),
                ),
              ),
            )
          else
            const SizedBox(height: 12),
          Row(
            children: [
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      count == null || count.total == 0
                          ? 'Plan'
                          : 'Plan · ${count.done} of ${count.total} done',
                      key: const ValueKey('plan-count'),
                      style: theme.textTheme.labelLarge?.copyWith(
                        color: scheme.onSurfaceVariant,
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      buildHeadline(p, s),
                      key: const ValueKey('plan-headline'),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: theme.textTheme.titleMedium,
                    ),
                  ],
                ),
              ),
              ?action,
            ],
          ),
          const SizedBox(height: 10),
          ClipRRect(
            borderRadius: BorderRadius.circular(2),
            child: LinearProgressIndicator(
              minHeight: 4,
              value: count == null || count.total == 0
                  ? (going ? null : 0)
                  : count.done / count.total,
              backgroundColor: scheme.surfaceContainerHighest,
            ),
          ),
          const SizedBox(height: 6),
        ],
      ),
    );
  }
}

/// [plan]'s features in build order: done (checked), doing (a spinner and
/// what the agent is doing now, from [state]), todo, dropped (struck
/// through), under [header].
///
/// With [plans] and [planKey] the person edits the plan as it builds: a tap
/// on a feature still to build edits its title and detail, a swipe drops
/// it (a tap on a dropped one takes the drop back), its handle reorders
/// it, and the field at the end adds one. Features the person added are
/// marked "you".
class PlanChecklist extends StatefulWidget {
  const PlanChecklist({
    super.key,
    required this.plan,
    required this.state,
    this.plans,
    this.planKey,
    this.controller,
    this.header,
  });

  final Plan? plan;
  final BuildState? state;
  final PlanStore? plans;
  final String? planKey;
  final ScrollController? controller;
  final Widget? header;

  @override
  State<PlanChecklist> createState() => _PlanChecklistState();
}

class _PlanChecklistState extends State<PlanChecklist> {
  final _add = TextEditingController();

  /// Features dropped here whose drop the plan does not show yet.
  final _dropping = <String>{};

  /// The order a reorder here asked for, until the plan shows it.
  List<String>? _order;

  bool get _editable => widget.plans != null && widget.planKey != null;

  @override
  void didUpdateWidget(PlanChecklist old) {
    super.didUpdateWidget(old);
    if (!identical(old.plan, widget.plan)) {
      _order = null;
      _dropping.removeWhere(
        (id) => widget.plan?.feature(id)?.status != FeatureStatus.todo,
      );
    }
  }

  @override
  void dispose() {
    _add.dispose();
    super.dispose();
  }

  List<PlanFeature> get _features {
    final fs = [
      for (final f in widget.plan?.features ?? const <PlanFeature>[])
        _dropping.contains(f.id)
            ? f.copyWith(status: FeatureStatus.dropped)
            : f,
    ];
    final order = _order;
    if (order == null) return fs;
    final byId = {for (final f in fs) f.id: f};
    return [
      for (final id in order) ?byId[id],
      for (final f in fs)
        if (!order.contains(f.id)) f,
    ];
  }

  Future<void> _submit() async {
    final text = _add.text.trim();
    if (text.isEmpty) return;
    _add.clear();
    final title = featureTitle(text);
    await widget.plans!.addFeature(
      widget.planKey!,
      title: title,
      detail: title == text ? '' : text,
    );
  }

  void _drop(PlanFeature f) {
    setState(() => _dropping.add(f.id));
    unawaited(
      widget.plans!.drop(widget.planKey!, f.id).catchError((Object _) {
        if (mounted) setState(() => _dropping.remove(f.id));
        return widget.plan!;
      }),
    );
  }

  void _reorder(int from, int to) {
    final fs = _features;
    if (from < 0 || from >= fs.length) return;
    final moved = fs.removeAt(from);
    fs.insert(to.clamp(0, fs.length), moved);
    final ids = [for (final f in fs) f.id];
    setState(() => _order = ids);
    unawaited(
      widget.plans!.reorder(widget.planKey!, ids).catchError((Object _) {
        if (mounted) setState(() => _order = null);
        return widget.plan!;
      }),
    );
  }

  /// Takes a drop back: the feature is to build again.
  Future<void> _restore(PlanFeature f) async {
    await widget.plans!.mark(
      widget.planKey!,
      f.id,
      FeatureStatus.todo,
      byPerson: true,
    );
  }

  Future<void> _edit(PlanFeature f) async {
    final edited = await showDialog<({String title, String detail})>(
      context: context,
      builder: (_) => _EditFeatureDialog(feature: f),
    );
    if (edited == null || !mounted) return;
    await widget.plans!.editFeature(
      widget.planKey!,
      f.id,
      title: edited.title,
      detail: edited.detail,
    );
  }

  @override
  Widget build(BuildContext context) {
    final fs = _features;
    final header = widget.header;
    final empty = fs.isEmpty;
    return ReorderableListView.builder(
      scrollController: widget.controller,
      buildDefaultDragHandles: false,
      padding: const EdgeInsets.only(bottom: 16),
      header: header == null && !empty
          ? null
          : Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [?header, if (empty) _empty(context)],
            ),
      footer: _editable ? _addField(context) : null,
      itemCount: fs.length,
      onReorderItem: _editable ? _reorder : (_, _) {},
      proxyDecorator: (child, _, _) => Material(
        elevation: 4,
        color: Theme.of(context).colorScheme.surfaceContainerHigh,
        borderRadius: BorderRadius.circular(12),
        child: child,
      ),
      itemBuilder: (context, i) => _row(context, fs[i], i),
    );
  }

  Widget _empty(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 16, 20, 8),
      child: Text(
        widget.plan == null
            ? 'The features show here once Clax has planned them.'
            : 'No features yet.',
        style: theme.textTheme.bodyMedium?.copyWith(
          color: theme.colorScheme.onSurfaceVariant,
        ),
      ),
    );
  }

  Widget _row(BuildContext context, PlanFeature f, int index) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final doing = f.status == FeatureStatus.doing;
    final todo = f.status == FeatureStatus.todo;
    final muted =
        f.status == FeatureStatus.done || f.status == FeatureStatus.dropped;
    final narration = doing ? buildStep(widget.state?.progress) : null;
    final subtitle = narration ?? (todo || doing ? f.detail : '');
    final leading = switch (f.status) {
      FeatureStatus.done => Icon(
        Icons.check_circle_rounded,
        color: scheme.primary,
        size: 22,
      ),
      FeatureStatus.doing => SizedBox.square(
        dimension: 22,
        child: Padding(
          padding: const EdgeInsets.all(2),
          child: CircularProgressIndicator(
            strokeWidth: 2.5,
            color: scheme.primary,
          ),
        ),
      ),
      FeatureStatus.todo => Icon(
        Icons.radio_button_unchecked_rounded,
        color: scheme.outline,
        size: 22,
      ),
      FeatureStatus.dropped => Icon(
        Icons.remove_circle_outline_rounded,
        color: scheme.outline,
        size: 22,
      ),
    };
    final tile = ListTile(
      contentPadding: const EdgeInsets.only(left: 20, right: 8),
      minLeadingWidth: 22,
      horizontalTitleGap: 14,
      leading: leading,
      title: Text.rich(
        TextSpan(
          children: [
            TextSpan(text: f.title),
            if (f.by == FeatureBy.person)
              TextSpan(
                text: '  you',
                style: theme.textTheme.labelSmall?.copyWith(
                  color: scheme.tertiary,
                  decoration: TextDecoration.none,
                  fontWeight: FontWeight.w600,
                ),
              ),
          ],
        ),
        maxLines: 2,
        overflow: TextOverflow.ellipsis,
        style: theme.textTheme.bodyLarge?.copyWith(
          color: muted ? scheme.onSurfaceVariant : null,
          fontWeight: doing ? FontWeight.w600 : null,
          decoration: f.status == FeatureStatus.dropped
              ? TextDecoration.lineThrough
              : null,
        ),
      ),
      subtitle: subtitle.isEmpty
          ? null
          : Text(
              subtitle,
              key: narration != null
                  ? const ValueKey('feature-narration')
                  : null,
              maxLines: doing ? 2 : 1,
              overflow: TextOverflow.ellipsis,
              style: theme.textTheme.bodyMedium?.copyWith(
                color: narration != null
                    ? scheme.primary
                    : scheme.onSurfaceVariant,
              ),
            ),
      trailing: _editable && todo
          ? ReorderableDragStartListener(
              index: index,
              child: Padding(
                padding: const EdgeInsets.all(12),
                child: Icon(
                  Icons.drag_handle_rounded,
                  key: ValueKey('reorder-${f.id}'),
                  color: scheme.onSurfaceVariant,
                ),
              ),
            )
          : null,
      onTap: !_editable
          ? null
          : todo
          ? () => unawaited(_edit(f))
          : f.status == FeatureStatus.dropped && !_dropping.contains(f.id)
          ? () => unawaited(_restore(f))
          : null,
    );
    final row = Material(
      type: MaterialType.transparency,
      child: Semantics(
        label: switch (f.status) {
          FeatureStatus.done => 'Done',
          FeatureStatus.doing => 'Building now',
          FeatureStatus.todo => 'To build',
          FeatureStatus.dropped => 'Dropped',
        },
        child: tile,
      ),
    );
    if (!_editable || !todo) {
      return KeyedSubtree(key: ValueKey('feature-${f.id}'), child: row);
    }
    return Dismissible(
      key: ValueKey('feature-${f.id}'),
      direction: DismissDirection.endToStart,
      onDismissed: (_) => _drop(f),
      background: ColoredBox(
        color: scheme.errorContainer,
        child: Align(
          alignment: Alignment.centerRight,
          child: Padding(
            padding: const EdgeInsets.only(right: 24),
            child: Text(
              'Drop',
              style: theme.textTheme.labelLarge?.copyWith(
                color: scheme.onErrorContainer,
              ),
            ),
          ),
        ),
      ),
      child: row,
    );
  }

  Widget _addField(BuildContext context) => Padding(
    key: const ValueKey('add-feature'),
    padding: const EdgeInsets.fromLTRB(16, 8, 16, 8),
    child: TextField(
      key: const ValueKey('add-feature-field'),
      controller: _add,
      textInputAction: TextInputAction.done,
      textCapitalization: TextCapitalization.sentences,
      onSubmitted: (_) => unawaited(_submit()),
      decoration: InputDecoration(
        hintText: 'Add a feature',
        prefixIcon: const Icon(Icons.add_rounded),
        border: OutlineInputBorder(borderRadius: BorderRadius.circular(16)),
        isDense: true,
        suffixIcon: IconButton(
          key: const ValueKey('add-feature-submit'),
          tooltip: 'Add',
          onPressed: () => unawaited(_submit()),
          icon: const Icon(Icons.arrow_upward_rounded),
        ),
      ),
    ),
  );
}

class _EditFeatureDialog extends StatefulWidget {
  const _EditFeatureDialog({required this.feature});

  final PlanFeature feature;

  @override
  State<_EditFeatureDialog> createState() => _EditFeatureDialogState();
}

class _EditFeatureDialogState extends State<_EditFeatureDialog> {
  late final _title = TextEditingController(text: widget.feature.title);
  late final _detail = TextEditingController(text: widget.feature.detail);

  @override
  void dispose() {
    _title.dispose();
    _detail.dispose();
    super.dispose();
  }

  void _save() {
    final t = _title.text.trim();
    if (t.isEmpty) return;
    Navigator.of(context).pop((title: t, detail: _detail.text.trim()));
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('Edit feature'),
    content: Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        TextField(
          key: const ValueKey('edit-feature-title'),
          controller: _title,
          autofocus: true,
          decoration: const InputDecoration(labelText: 'Feature'),
        ),
        const SizedBox(height: 12),
        TextField(
          key: const ValueKey('edit-feature-detail'),
          controller: _detail,
          minLines: 2,
          maxLines: 4,
          decoration: const InputDecoration(labelText: 'What it does'),
        ),
      ],
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.of(context).pop(),
        child: const Text('Cancel'),
      ),
      FilledButton(
        key: const ValueKey('edit-feature-save'),
        onPressed: _save,
        child: const Text('Save'),
      ),
    ],
  );
}

/// Shows app [aid]'s build over the app: the plan panel in a draggable
/// sheet, with Stop and Resume.
Future<void> showBuildSheet(BuildContext context, BuildLink link, String aid) =>
    showModalBottomSheet<void>(
      context: context,
      isScrollControlled: true,
      useSafeArea: true,
      showDragHandle: false,
      builder: (_) => DraggableScrollableSheet(
        expand: false,
        initialChildSize: 0.6,
        minChildSize: 0.3,
        maxChildSize: 0.92,
        builder: (context, controller) => BuildWatch(
          key: const ValueKey('build-sheet'),
          link: link,
          appId: aid,
          builder: (context, plan, state) => BuildPlanPanel(
            plan: plan,
            state: state,
            plans: link.plans,
            planKey: aid,
            controller: controller,
            onStop: () => unawaited(link.control.stopBuild(aid)),
            onResume: () => unawaited(link.control.resumeBuild(aid)),
          ),
        ),
      ),
    );
