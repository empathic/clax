/// Each app's build plan: the features the agent builds in order, which
/// the person can edit while it builds, and the agent's `plan_set` and
/// `plan_mark` tools.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import '../platform/clock.dart';
import 'tools/tool_spec.dart';
import 'tools/toolbox.dart';

/// Where a feature of a plan stands.
enum FeatureStatus { todo, doing, done, dropped }

/// Who added a feature to a plan.
enum FeatureBy { agent, person }

/// One feature of a plan.
class PlanFeature {
  const PlanFeature({
    required this.id,
    required this.title,
    this.detail = '',
    this.status = FeatureStatus.todo,
    this.by = FeatureBy.agent,
    this.droppedByPerson = false,
  });

  factory PlanFeature.fromJson(Map<String, Object?> j) => PlanFeature(
    id: j['id']! as String,
    title: j['title']! as String,
    detail: j['detail'] as String? ?? '',
    status: FeatureStatus.values.byName(j['status'] as String? ?? 'todo'),
    by: FeatureBy.values.byName(j['by'] as String? ?? 'agent'),
    droppedByPerson: j['dropped_by_person'] as bool? ?? false,
  );

  /// Short and unique within its plan (`f1`, `export`).
  final String id;
  final String title;

  /// What the feature does, in a sentence or two.
  final String detail;
  final FeatureStatus status;

  /// Who added it. The agent's `plan_set` never removes a feature the
  /// person added.
  final FeatureBy by;

  /// The person dropped it: the agent's tools never take it up again.
  final bool droppedByPerson;

  PlanFeature copyWith({
    String? title,
    String? detail,
    FeatureStatus? status,
    FeatureBy? by,
    bool? droppedByPerson,
  }) => PlanFeature(
    id: id,
    title: title ?? this.title,
    detail: detail ?? this.detail,
    status: status ?? this.status,
    by: by ?? this.by,
    droppedByPerson: droppedByPerson ?? this.droppedByPerson,
  );

  Map<String, Object?> toJson() => {
    'id': id,
    'title': title,
    'detail': detail,
    'status': status.name,
    'by': by.name,
    if (droppedByPerson) 'dropped_by_person': true,
  };

  @override
  bool operator ==(Object other) =>
      other is PlanFeature &&
      other.id == id &&
      other.title == title &&
      other.detail == detail &&
      other.status == status &&
      other.by == by &&
      other.droppedByPerson == droppedByPerson;

  @override
  int get hashCode =>
      Object.hash(id, title, detail, status, by, droppedByPerson);

  @override
  String toString() => 'PlanFeature($id, $title, ${status.name})';
}

/// One app's plan: the person's [request] and its [features] in build
/// order.
///
/// Beside the contract's fields, a plan keeps its build's bookkeeping:
/// [revision] (one more at each write, so a run can tell the person
/// edited the plan since it last saw it), [spentUsd] (the cost of the
/// build's runs so far, against [buildCapUsd]) and [stopped] (the person
/// stopped the build).
class Plan {
  const Plan({
    required this.request,
    required this.features,
    required this.updatedAt,
    this.revision = 1,
    this.spentUsd = 0,
    this.stopped = false,
  });

  factory Plan.fromJson(Map<String, Object?> j) => Plan(
    request: j['request'] as String? ?? '',
    features: [
      for (final f in j['features'] as List? ?? const [])
        PlanFeature.fromJson((f as Map).cast<String, Object?>()),
    ],
    updatedAt: DateTime.parse(j['updatedAt']! as String),
    revision: j['revision'] as int? ?? 1,
    spentUsd: (j['spentUsd'] as num?)?.toDouble() ?? 0,
    stopped: j['stopped'] as bool? ?? false,
  );

  final String request;
  final List<PlanFeature> features;
  final DateTime updatedAt;
  final int revision;
  final double spentUsd;
  final bool stopped;

  /// The features still to build, in order.
  List<PlanFeature> get todo => [
    for (final f in features)
      if (f.status == FeatureStatus.todo || f.status == FeatureStatus.doing) f,
  ];

  /// The feature being built now, or null.
  PlanFeature? get doing =>
      features.where((f) => f.status == FeatureStatus.doing).firstOrNull;

  PlanFeature? feature(String id) =>
      features.where((f) => f.id == id).firstOrNull;

  Plan copyWith({
    String? request,
    List<PlanFeature>? features,
    DateTime? updatedAt,
    int? revision,
    double? spentUsd,
    bool? stopped,
  }) => Plan(
    request: request ?? this.request,
    features: features ?? this.features,
    updatedAt: updatedAt ?? this.updatedAt,
    revision: revision ?? this.revision,
    spentUsd: spentUsd ?? this.spentUsd,
    stopped: stopped ?? this.stopped,
  );

  Map<String, Object?> toJson() => {
    'request': request,
    'features': [for (final f in features) f.toJson()],
    'updatedAt': updatedAt.toUtc().toIso8601String(),
    'revision': revision,
    'spentUsd': spentUsd,
    'stopped': stopped,
  };

  /// The form a run's message and the plan tools' results carry: the
  /// features, one JSON object per line.
  String toPromptLines() =>
      [for (final f in features) jsonEncode(f.toJson())].join('\n');
}

/// What a whole build (the first run and its continue runs) may cost, in
/// US dollars.
const buildCapUsd = 6.0;

/// The plans of every app, by app ID, kept in [file] (as JSON, written to
/// a temporary file beside it and renamed into place) or, without one, in
/// memory. A file that cannot be read is treated as empty.
///
/// A new app's first run plans before the app has an ID: it keeps its plan
/// under a provisional key, which [move] renames to the app's ID once it
/// is created.
class PlanStore {
  PlanStore({this.file, this.clock = const SystemClock()});

  final File? file;
  final Clock clock;

  final _plans = <String, Plan>{};
  final _watchers = <String, StreamController<Plan?>>{};
  Future<void>? _loaded;
  Future<void> _saving = Future.value();

  Future<void> _load() => _loaded ??= () async {
    final f = file;
    if (f == null) return;
    try {
      if (!await f.exists()) return;
      final j = jsonDecode(await f.readAsString()) as Map;
      for (final e in (j['plans'] as Map).entries) {
        _plans[e.key as String] = Plan.fromJson(
          (e.value as Map).cast<String, Object?>(),
        );
      }
    } on Object {
      // An unreadable file loses the plans, not the apps.
    }
  }();

  Future<void> _save() {
    final f = file;
    if (f == null) return Future.value();
    final json = jsonEncode({
      'plans': {for (final e in _plans.entries) e.key: e.value.toJson()},
    });
    return _saving = _saving
        .then((_) async {
          await f.parent.create(recursive: true);
          final tmp = File('${f.path}.tmp');
          await tmp.writeAsString(json, flush: true);
          await tmp.rename(f.path);
        })
        .catchError((Object _) {});
  }

  /// The plan of [appId], or null.
  Future<Plan?> get(String appId) async {
    await _load();
    return _plans[appId];
  }

  /// Every plan, by app ID.
  Future<Map<String, Plan>> all() async {
    await _load();
    return Map.of(_plans);
  }

  /// The plan of [appId] now and after each change; null while it has
  /// none.
  Stream<Plan?> watch(String appId) {
    final c = _watchers.putIfAbsent(
      appId,
      () => StreamController<Plan?>.broadcast(),
    );
    late final StreamController<Plan?> out;
    StreamSubscription<Plan?>? sub;
    out = StreamController<Plan?>(
      onListen: () async {
        sub = c.stream.listen(out.add);
        out.add(await get(appId));
      },
      onCancel: () => sub?.cancel(),
    );
    return out.stream;
  }

  Future<Plan> _write(String appId, Plan Function(Plan? old) change) async {
    await _load();
    final old = _plans[appId];
    final next = change(old)
        .copyWith(updatedAt: clock.now(), revision: (old?.revision ?? 0) + 1);
    _plans[appId] = next;
    _watchers[appId]?.add(next);
    await _save();
    return next;
  }

  Plan _need(String appId, Plan? p) =>
      p ?? (throw StateError('app $appId has no plan'));

  /// Sets the whole feature list of [appId] (the agent's `plan_set`),
  /// keeping the request, cost and stop of a plan it already has. With
  /// [freshBuild] (a change in answer to the person's comment planned a big
  /// request), a plan with nothing left to build that gains features to
  /// build starts a new build: its cost and stop are cleared, so the new
  /// build has its own [buildCapUsd]. Without it (a build's own runs) the
  /// cost is kept, so a build never renews its own budget.
  Future<Plan> set(
    String appId,
    List<PlanFeature> features, {
    String request = '',
    bool freshBuild = false,
  }) => _write(appId, (old) {
    if (old == null) {
      return Plan(request: request, features: features, updatedAt: clock.now());
    }
    final fresh =
        freshBuild &&
        old.todo.isEmpty &&
        features.any(
          (f) =>
              f.status == FeatureStatus.todo || f.status == FeatureStatus.doing,
        );
    return old.copyWith(
      features: features,
      spentUsd: fresh ? 0 : null,
      stopped: fresh ? false : null,
    );
  });

  /// Sets the status of feature [id]; throws [StateError] when there is no
  /// such feature. [byPerson]: the person set it, so a drop is theirs and
  /// any other status takes their drop back.
  Future<Plan> mark(
    String appId,
    String id,
    FeatureStatus status, {
    bool byPerson = false,
  }) => _write(appId, (old) {
    final p = _need(appId, old);
    if (p.feature(id) == null) throw StateError('no feature "$id"');
    return p.copyWith(
      features: [
        for (final f in p.features)
          f.id == id
              ? f.copyWith(
                  status: status,
                  droppedByPerson: byPerson
                      ? status == FeatureStatus.dropped
                      : null,
                )
              : f,
      ],
    );
  });

  /// Adds a feature to build after the others; returns it.
  Future<PlanFeature> addFeature(
    String appId, {
    required String title,
    String detail = '',
  }) async {
    late PlanFeature added;
    await _write(appId, (old) {
      final p =
          old ?? Plan(request: '', features: const [], updatedAt: clock.now());
      var n = p.features.length + 1;
      while (p.feature('p$n') != null) {
        n++;
      }
      added = PlanFeature(
        id: 'p$n',
        title: title,
        detail: detail,
        by: FeatureBy.person,
      );
      return p.copyWith(features: [...p.features, added]);
    });
    return added;
  }

  /// Changes the title or detail of feature [id].
  Future<Plan> editFeature(
    String appId,
    String id, {
    String? title,
    String? detail,
  }) => _write(appId, (old) {
    final p = _need(appId, old);
    return p.copyWith(
      features: [
        for (final f in p.features)
          f.id == id ? f.copyWith(title: title, detail: detail) : f,
      ],
    );
  });

  /// The person drops feature [id]: it stays in the list, and is not
  /// built; the agent cannot take it up again.
  Future<Plan> drop(String appId, String id) =>
      mark(appId, id, FeatureStatus.dropped, byPerson: true);

  /// Puts the features in the order of [ids]; features not named keep
  /// their order after them.
  Future<Plan> reorder(String appId, List<String> ids) => _write(appId, (old) {
    final p = _need(appId, old);
    final named = [for (final id in ids) ?p.feature(id)];
    return p.copyWith(
      features: [
        ...named,
        for (final f in p.features)
          if (!ids.contains(f.id)) f,
      ],
    );
  });

  /// Adds [usd] to the build's cost.
  Future<Plan?> spend(String appId, double usd) async {
    if (await get(appId) == null) return null;
    return _write(appId, (old) => old!.copyWith(spentUsd: old.spentUsd + usd));
  }

  /// Starts a new build budget of [appId]: its cost goes back to 0, at the
  /// person's own action (Resume of a build that spent [buildCapUsd]).
  Future<Plan?> freshBudget(String appId) async {
    if (await get(appId) == null) return null;
    return _write(appId, (old) => old!.copyWith(spentUsd: 0));
  }

  /// Marks the build stopped or not; a stopped build's features that were
  /// being built go back to todo.
  Future<Plan?> setStopped(String appId, bool stopped) async {
    if (await get(appId) == null) return null;
    return _write(
      appId,
      (old) => old!.copyWith(
        stopped: stopped,
        features: [
          for (final f in old.features)
            stopped && f.status == FeatureStatus.doing
                ? f.copyWith(status: FeatureStatus.todo)
                : f,
        ],
      ),
    );
  }

  /// Renames the plan kept under [from] (a first run's provisional key) to
  /// [to], the app's ID.
  Future<void> move(String from, String to) async {
    await _load();
    final p = _plans.remove(from);
    if (p == null) return;
    _plans[to] = p;
    _watchers[from]?.add(null);
    _watchers[to]?.add(p);
    await _save();
  }

  /// Forgets the plan of [appId].
  Future<void> remove(String appId) async {
    await _load();
    if (_plans.remove(appId) == null) return;
    _watchers[appId]?.add(null);
    await _save();
  }
}

/// Which run of a build this is.
enum RunKind {
  /// A new app's first run: plan, then a usable first version.
  first,

  /// A run the worker queues after the first, building the next features.
  continueBuild,

  /// A change in answer to comments.
  change,
}

/// The plan as one run of the agent sees it: the store, the key the run's
/// plan is kept under, and the plan revision the run last saw, so
/// `plan_set` never writes over an edit the person made meanwhile.
class RunPlan {
  RunPlan(
    this.store,
    this.key, {
    required this.request,
    this.seen,
    this.freshBuild = false,
  });

  final PlanStore store;

  /// Whether this run's `plan_set` may start a new build on a plan with
  /// nothing left to build ([PlanStore.set]): only a change in answer to
  /// the person's comment.
  final bool freshBuild;

  /// The app's ID, or a first run's provisional key.
  String key;

  /// The person's request, for a plan made by this run.
  final String request;

  /// The revision of the plan this run was last shown; null when it was
  /// shown none.
  int? seen;
}

/// The agent's `plan_set`: the whole feature list, the first plan or a
/// revision.
class PlanSetTool implements Tool {
  PlanSetTool(this.plan);

  final RunPlan plan;

  @override
  final spec = ToolSpec(
    'plan_set',
    "Sets the app's whole plan: every feature in build order, each with a "
        'short unique id, a title of a few words, a detail of one or two '
        'sentences saying what it does, and its status (todo, doing, done, '
        'dropped). Keep the ids and statuses of features already in the '
        'plan. Refused when the person changed the plan since you last saw '
        'it: the result then holds the plan as it is now.',
    objectSchema({
      'features': {
        'type': 'array',
        'items': objectSchema({
          'id': {'type': 'string'},
          'title': {'type': 'string'},
          'detail': {'type': 'string'},
          'status': {
            'type': 'string',
            'enum': [for (final s in FeatureStatus.values) s.name],
          },
        }),
      },
    }),
  );

  @override
  Future<ToolOutput> call(Map<String, Object?> input, RunState s) async {
    final raw = input['features'];
    if (raw is! List || raw.isEmpty) {
      throw const ToolInputError('features must be a non-empty list.');
    }
    final features = <PlanFeature>[];
    final ids = <String>{};
    for (final f in raw) {
      if (f is! Map) throw const ToolInputError('each feature is an object.');
      final id = f['id'];
      final title = f['title'];
      if (id is! String || id.trim().isEmpty) {
        throw const ToolInputError('each feature needs an id.');
      }
      if (!ids.add(id.trim())) {
        throw ToolInputError('the id "$id" is given twice.');
      }
      if (title is! String || title.trim().isEmpty) {
        throw ToolInputError('feature "$id" needs a title.');
      }
      final status = FeatureStatus.values
          .where((v) => v.name == f['status'])
          .firstOrNull;
      if (status == null) {
        throw ToolInputError(
          'feature "$id": status is one of todo, doing, done, dropped.',
        );
      }
      features.add(
        PlanFeature(
          id: id.trim(),
          title: title.trim(),
          detail: (f['detail'] as String? ?? '').trim(),
          status: status,
        ),
      );
    }
    final current = await plan.store.get(plan.key);
    if (current != null && current.revision != plan.seen) {
      plan.seen = current.revision;
      return ToolRefused(
        'The person changed the plan since you last saw it. It is now, one '
        'feature per line:\n${current.toPromptLines()}\nCall plan_set again '
        'from this plan if it still needs changing.',
      );
    }
    // The person's features stay, and their drops stay dropped; who added
    // a feature is kept from the plan, never taken from the agent.
    final kept = <PlanFeature>[];
    for (final f in features) {
      final was = current?.feature(f.id);
      if (was != null &&
          was.droppedByPerson &&
          f.status != FeatureStatus.dropped) {
        return ToolRefused(
          'The person dropped "${was.title}" (${was.id}): keep it dropped.',
        );
      }
      kept.add(
        f.copyWith(
          by: was?.by ?? FeatureBy.agent,
          droppedByPerson: was?.droppedByPerson ?? false,
        ),
      );
    }
    final missing = [
      for (final f in current?.features ?? const <PlanFeature>[])
        if (f.by == FeatureBy.person && !ids.contains(f.id)) f,
    ];
    if (missing.isNotEmpty) {
      return ToolRefused(
        'The person added ${[for (final f in missing) '"${f.title}" (${f.id})'].join(', ')}: '
        'keep every feature they added (mark one dropped only when it cannot '
        'be done here).',
      );
    }
    final saved = await plan.store.set(
      plan.key,
      kept,
      request: plan.request,
      freshBuild: plan.freshBuild,
    );
    plan.seen = saved.revision;
    s.planned = saved.todo.isNotEmpty;
    return ToolOk({
      'features': saved.features.length,
      'todo': saved.todo.length,
    });
  }
}

/// The agent's `plan_mark`: one feature's status.
class PlanMarkTool implements Tool {
  PlanMarkTool(this.plan);

  final RunPlan plan;

  @override
  final spec = ToolSpec(
    'plan_mark',
    "Sets one plan feature's status: doing when you start it, done once it "
        'works, is tested and rendered clean, dropped when it cannot be done.',
    objectSchema({
      'id': {'type': 'string'},
      'status': {
        'type': 'string',
        'enum': [for (final s in FeatureStatus.values) s.name],
      },
    }),
  );

  @override
  Future<ToolOutput> call(Map<String, Object?> input, RunState s) async {
    final id = input['id'];
    final status = FeatureStatus.values
        .where((v) => v.name == input['status'])
        .firstOrNull;
    if (id is! String || status == null) {
      throw const ToolInputError(
        'plan_mark takes id and status (todo, doing, done, dropped).',
      );
    }
    final before = await plan.store.get(plan.key);
    if (before?.feature(id)?.droppedByPerson ?? false) {
      throw ToolInputError(
        'The person dropped "$id": leave it dropped and build the next one.',
      );
    }
    if (before?.feature(id) == null) {
      throw ToolInputError(
        before == null
            ? 'There is no plan yet: call plan_set first.'
            : 'There is no feature "$id"; the plan is:\n'
                  '${before.toPromptLines()}',
      );
    }
    final saved = await plan.store.mark(plan.key, id, status);
    // Seen still holds when nothing else wrote the plan in between.
    if (plan.seen == before!.revision) plan.seen = saved.revision;
    final next = saved.todo.firstOrNull;
    return ToolOk({'id': id, 'status': status.name, 'next': next?.id});
  }
}
