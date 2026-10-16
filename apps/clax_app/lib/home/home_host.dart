/// The home screen's sources, owned once for the built-in home screen
/// (`Launcher`) and the home app alike: the owner's apps, the makes going
/// on, the builds' progress and the inbox's unread count.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:ui' show Rect;

import 'package:flutter/foundation.dart';

import '../agent/plan.dart';
import '../agent/worker.dart';
import '../clax/api.dart';
import '../ui/app_icon.dart';
import '../ui/build_plan.dart';
import '../ui/inbox_screen.dart' show resubscribe;
import '../ui/launcher.dart'
    show EmojiFace, IconFace, MonogramFace, tileFace, tileHue;
import '../ui/make_service.dart';
import 'home_settings.dart';

/// One app on the home screen.
///
/// [icon] is one of `{url}` (its own icon, as a `data:` URL of the cleaned
/// SVG), `{emoji}`, `{word}` (an icon word the home screen draws) or
/// `{monogram, hue}` (one or two letters on a hue of 0 to 359). [build] is
/// how far its build is while one goes.
class HomeApp {
  const HomeApp({
    required this.id,
    required this.title,
    required this.updatedAt,
    required this.icon,
    this.build,
  });

  final String id, title;
  final DateTime updatedAt;
  final Map<String, Object?> icon;
  final ({int done, int total})? build;

  Map<String, Object?> toJson() => {
    'id': id,
    'title': title,
    'updatedAt': updatedAt.toUtc().toIso8601String(),
    'icon': icon,
    'build': _count(build),
  };
}

/// One make going on, waiting, interrupted or failed.
class HomeMake {
  const HomeMake({
    required this.id,
    required this.request,
    required this.state,
    this.progress,
  });

  final String id, request;

  /// `making`, `offline` (waiting for the Claude API's connection),
  /// `waiting` (for a slot), `interrupted` or `failed`.
  final String state;

  /// The features its plan has done, once the plan has any.
  final ({int done, int total})? progress;

  Map<String, Object?> toJson() => {
    'id': id,
    'request': request,
    'state': state,
    'progress': _count(progress),
  };
}

Map<String, Object?>? _count(({int done, int total})? c) =>
    c == null ? null : {'done': c.done, 'total': c.total};

/// What the home screen shows: [apps] newest first (the home app left
/// out), [makes] newest first, the inbox's [unread] count, and whether the
/// API key is missing ([needsKey]).
class HomeState {
  const HomeState({
    required this.apps,
    required this.makes,
    required this.unread,
    required this.needsKey,
  });

  final List<HomeApp> apps;
  final List<HomeMake> makes;
  final int unread;
  final bool needsKey;
}

/// What the home screen asks of the app's screens: each opens its screen
/// or sheet over the home screen.
abstract class HomeActions {
  /// Opens app [id], as a tile's tap does.
  void open(String id);

  /// Shows make [id]'s build view, or asks "Make it again?" for an
  /// interrupted one.
  void openMake(String id);

  /// Shows the New app sheet.
  void newApp();

  /// Shows app [id]'s long-press menu, anchored at [anchor].
  void menu(String id, Rect anchor);

  void openInbox();
  void openSettings();
  void importApp();
}

/// A `home` call from a caller that is not the home app.
class HomeRefused implements Exception {
  const HomeRefused(this.caller);
  final String caller;
  @override
  String toString() => 'HomeRefused: $caller is not the home app';
}

/// A `home` call with a method or arguments the capability does not take.
class HomeBadRequest implements Exception {
  const HomeBadRequest(this.message);
  final String message;
  @override
  String toString() => 'HomeBadRequest: $message';
}

/// One change for the home app: [name] is `apps`, `makes`, `unread` or
/// `key`, and [data] the topic's whole latest value (as `apps()`,
/// `makes()`, `unread()` or `needsKey()` would answer).
typedef HomeEvent = ({String name, Object? data});

/// Follows `gallery` and `inbox` on [stream] (the app's shared stream; it
/// is closed on [dispose] only when [closeStream]), [makes] and [builds],
/// and holds the latest [state]. Once subscribed it loads the list and
/// [ClaxApi.inboxSummary]; then each `gallery` `version` refreshes its
/// app, each `artifact_deleted` removes one, and each `inbox` event
/// carries the unread count. A topic's `resync` loads it again, as does a
/// topic the stream dropped (`unsubscribed`), once subscribed again.
///
/// Listeners hear of each change; [events] carries each change of a
/// topic's value for the home app, the latest value only, so a page that
/// was covered catches up from the next one or by asking.
///
/// [call] answers the home app's `home` calls, and only the home app's:
/// a caller whose artifact ID is not [HomeSettings.homeAppId] gets
/// [HomeRefused].
class HomeHost extends ChangeNotifier {
  HomeHost({
    required this.api,
    required this.stream,
    required this.makes,
    required this.builds,
    required this.settings,
    required this.actions,
    this.icons,
    this.retryDelay,
    this.closeStream = false,
    this._needsKey = false,
  }) {
    makes.addListener(_onMakes);
    icons?.addListener(_onIcons);
    _events = stream.events.listen(_onEvent, onError: (Object _) {});
    if (builds case final b?) {
      _builds = b.control.builds.listen(_onBuild, onError: (Object _) {});
    }
    _watchMakePlans();
    unawaited(_watch());
  }

  final ClaxApi api;
  final ClaxStream stream;
  final MakeService makes;

  /// The plans and builds; null shows no build's progress.
  final BuildLink? builds;
  final HomeSettings settings;
  final HomeActions actions;

  /// The apps' icons, for `{url}` faces; null gives none.
  final AppIcons? icons;

  /// Waits between tries to subscribe a dropped topic again;
  /// [Future.delayed] when null.
  final Future<void> Function(Duration)? retryDelay;

  /// Whether [dispose] closes [stream].
  final bool closeStream;

  static const _topics = ['gallery', 'inbox'];

  StreamSubscription<StreamEvent>? _events;
  StreamSubscription<BuildState>? _builds;
  final _out = StreamController<HomeEvent>.broadcast(sync: true);
  var _dead = false;

  List<ArtifactSummary>? _summaries;
  Object? _loadError;
  int _unread = 0;
  bool _needsKey;
  int _loads = 0;
  int _counts = 0;
  final Set<String> _resubscribing = {};

  /// Each app's plan, while it is listed, and each build's latest step.
  final _plans = <String, Plan?>{};
  final _planSubs = <String, StreamSubscription<Plan?>>{};
  final _buildStates = <String, BuildState>{};

  /// Each make's plan, by its plan key.
  final _makePlans = <String, Plan?>{};
  final _makePlanSubs = <String, StreamSubscription<Plan?>>{};

  /// A stable ID for each make, given the first time it is seen.
  final _makeIds = Expando<String>();
  int _makeSeq = 0;

  /// What [events] last carried per topic, as JSON.
  final _told = <String, String>{};

  HomeState? _state;

  /// The latest of everything the home screen shows.
  HomeState get state => _state ??= HomeState(
    apps: [for (final a in summaries ?? const <ArtifactSummary>[]) _app(a)],
    makes: [for (final m in makes.makes) _make(m)],
    unread: _unread,
    needsKey: _needsKey,
  );

  /// The apps as the daemon lists them, newest first, the home app left
  /// out; null until the first load answered.
  List<ArtifactSummary>? get summaries {
    final home = settings.homeAppId;
    final all = _summaries;
    if (all == null || home == null) return all;
    return [
      for (final a in all)
        if (a.id != home) a,
    ];
  }

  /// Why the last load of the list failed, while it stands.
  Object? get loadError => _loadError;

  /// The inbox's unread count.
  int get unread => _unread;

  /// Each change of a topic for the home app (see [HomeEvent]).
  Stream<HomeEvent> get events => _out.stream;

  /// Whether the API key is missing.
  bool get needsKey => _needsKey;
  set needsKey(bool v) {
    if (v == _needsKey) return;
    _needsKey = v;
    _changed();
  }

  /// How far app [aid]'s build is, while one goes.
  ({int done, int total})? buildOf(String aid) {
    final plan = _plans[aid];
    final s = _buildStates[aid] ?? builds?.control.buildOf(aid);
    if (!buildGoing(s) || plan == null) return null;
    final c = planCount(plan);
    return c.total == 0 ? null : c;
  }

  /// Make [id], while it is listed.
  MakeSession? sessionOf(String id) {
    for (final m in makes.makes) {
      if (_makeIds[m] == id) return m;
    }
    return null;
  }

  /// The ID [state] gives make [s].
  String makeIdOf(MakeSession s) => _makeIds[s] ??= 'm${++_makeSeq}';

  /// Whether the `home` capability serves app [aid]: only the home app.
  bool availableTo(String aid) {
    final home = settings.homeAppId;
    return home != null && aid == home;
  }

  /// [settings] changed (the home app was installed, reset or switched);
  /// the lists leave the new home app out from now on.
  void settingsChanged() => _changed();

  /// Answers the home app's call [method] with [args] (named arguments,
  /// as the shell sends them). Throws [HomeRefused] when [callerAid] is
  /// not the home app, and [HomeBadRequest] for a method or arguments the
  /// capability does not take.
  Future<Object?> call(
    String callerAid,
    String method,
    Map<String, Object?> args,
  ) async {
    if (!availableTo(callerAid)) throw HomeRefused(callerAid);
    switch (method) {
      case 'apps':
        return _json('apps');
      case 'makes':
        return _json('makes');
      case 'unread':
        return _unread;
      case 'needsKey':
        return _needsKey;
      case 'open':
        actions.open(_appId(args));
      case 'openMake':
        final id = _string(args, 'id');
        if (sessionOf(id) == null) throw HomeBadRequest('no make $id');
        actions.openMake(id);
      case 'newApp':
        actions.newApp();
      case 'menu':
        actions.menu(_appId(args), _rect(args['rect']));
      case 'openInbox':
        actions.openInbox();
      case 'openSettings':
        actions.openSettings();
      case 'importApp':
        actions.importApp();
      default:
        throw HomeBadRequest('home.$method is not a home method');
    }
    return null;
  }

  static String _string(Map<String, Object?> args, String key) =>
      switch (args[key]) {
        final String s when s.isNotEmpty => s,
        _ => throw HomeBadRequest('takes {$key: string}'),
      };

  String _appId(Map<String, Object?> args) {
    final id = _string(args, 'id');
    if (!(summaries ?? const []).any((a) => a.id == id)) {
      throw HomeBadRequest('no app $id on the home screen');
    }
    return id;
  }

  static Rect _rect(Object? r) {
    if (r
        case {
          'x': final num x,
          'y': final num y,
          'w': final num w,
          'h': final num h,
        }
        when [x, y, w, h].every((v) => v.isFinite) && w >= 0 && h >= 0) {
      return Rect.fromLTWH(
        x.toDouble(),
        y.toDouble(),
        w.toDouble(),
        h.toDouble(),
      );
    }
    throw const HomeBadRequest('menu takes a rect {x, y, w, h}');
  }

  /// Topic [name]'s value as the page gets it.
  Object? _json(String name) => switch (name) {
    'apps' => [for (final a in state.apps) a.toJson()],
    'makes' => [for (final m in state.makes) m.toJson()],
    'unread' => state.unread,
    _ => state.needsKey,
  };

  HomeApp _app(ArtifactSummary a) => HomeApp(
    id: a.id,
    title: a.title,
    updatedAt:
        DateTime.tryParse(a.updatedAt) ??
        DateTime.fromMillisecondsSinceEpoch(0),
    icon: _icon(a),
    build: buildOf(a.id),
  );

  Map<String, Object?> _icon(ArtifactSummary a) {
    if (icons?.svgOf(a.id) case final svg?) {
      return {
        'url': 'data:image/svg+xml;base64,${base64.encode(utf8.encode(svg))}',
      };
    }
    return switch (tileFace(a)) {
      EmojiFace(:final emoji) => {'emoji': emoji},
      IconFace() => {'word': a.icon!.trim().toLowerCase()},
      MonogramFace(:final letters) => {
        'monogram': letters,
        'hue': tileHue(a.id),
      },
    };
  }

  HomeMake _make(MakeSession s) {
    final failed = s.failure != null && !s.running;
    final key = s.planKey;
    final plan = key == null ? null : _makePlans[key];
    final c = plan == null ? null : planCount(plan);
    return HomeMake(
      id: makeIdOf(s),
      request: s.request,
      state: failed
          ? 'failed'
          : s.running && s.waitingForConnection
          ? 'offline'
          : s.interrupted
          ? 'interrupted'
          : s.queued
          ? 'waiting'
          : 'making',
      progress: c == null || c.total == 0 ? null : c,
    );
  }

  /// Something the state is made of changed: listeners hear of it, and the
  /// home app of each topic whose value changed.
  void _changed() {
    if (_dead) return;
    _state = null;
    notifyListeners();
    if (!_out.hasListener) return;
    for (final (topic, name) in const [
      ('apps', 'apps'),
      ('makes', 'makes'),
      ('unread', 'unread'),
      ('needsKey', 'key'),
    ]) {
      final data = _json(topic);
      final s = jsonEncode(data);
      if (_told[name] == s) continue;
      _told[name] = s;
      _out.add((name: name, data: data));
    }
  }

  Future<void> _watch() async {
    try {
      await stream.subscribe(_topics);
    } catch (_) {
      // The list and count still load; they only miss live changes until
      // the next fetch.
    }
    if (_dead) return;
    await Future.wait([load(), loadUnread()]);
  }

  Future<void> _resubscribe(String topic) async {
    if (!_resubscribing.add(topic)) return;
    try {
      final ok = await resubscribe(
        stream,
        [topic],
        delay: retryDelay ?? Future.delayed,
        alive: () => !_dead,
      );
      if (ok && !_dead) await _reload(topic);
    } finally {
      _resubscribing.remove(topic);
    }
  }

  Future<void> _reload(String topic) async {
    if (topic == 'gallery') await load();
    if (topic == 'inbox') await loadUnread();
  }

  /// Loads the list again; only the newest load's answer is kept.
  Future<void> load() async {
    final n = ++_loads;
    try {
      final apps = await api.listArtifacts();
      if (_dead || n != _loads) return;
      _summaries = _newestFirst(apps);
      _loadError = null;
      _syncPlans();
      _changed();
    } catch (e) {
      if (_dead || n != _loads) return;
      _loadError = e;
      _changed();
    }
  }

  /// Loads the unread count again; a count an event carried meanwhile wins.
  Future<void> loadUnread() async {
    final n = ++_counts;
    try {
      final summary = await api.inboxSummary();
      if (_dead || n != _counts) return;
      _setUnread(summary.unread);
    } catch (_) {
      // Kept as it was; the next event or fetch corrects it.
    }
  }

  void _setUnread(int n) {
    if (n == _unread) return;
    _unread = n;
    _changed();
  }

  static List<ArtifactSummary> _newestFirst(List<ArtifactSummary> apps) =>
      List.of(apps)..sort((a, b) => b.updatedAt.compareTo(a.updatedAt));

  void _onEvent(StreamEvent e) {
    if (_dead) return;
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
          _counts++;
          _setUnread(n.toInt());
        }
    }
  }

  Future<void> _onGallery(StreamEvent e) async {
    final aid = e.data['artifact_id'];
    if (aid is! String) return;
    if (e.type == 'artifact_deleted') {
      _summaries = _summaries?.where((a) => a.id != aid).toList();
      _syncPlans();
      _changed();
      return;
    }
    try {
      final summary = (await api.getArtifact(aid)).artifact;
      if (_dead) return;
      _summaries = _newestFirst([
        summary,
        ...?_summaries?.where((a) => a.id != aid),
      ]);
      _syncPlans();
      _changed();
    } catch (_) {
      // Deleted again, or the daemon is restarting; the next load catches up.
    }
  }

  /// Watches the plan of each listed app, and stops watching the others.
  void _syncPlans() {
    final b = builds;
    if (b == null) return;
    final ids = {for (final a in _summaries ?? const <ArtifactSummary>[]) a.id};
    for (final gone in _planSubs.keys.where((k) => !ids.contains(k)).toList()) {
      unawaited(_planSubs.remove(gone)!.cancel());
      _plans.remove(gone);
    }
    for (final aid in ids) {
      _planSubs[aid] ??= b.plans.watch(aid).listen((p) {
        final had = _plans.containsKey(aid);
        final before = _plans[aid];
        _plans[aid] = p;
        if (had || p != null || before != null) _changed();
      }, onError: (Object _) {});
    }
  }

  void _onBuild(BuildState s) {
    if (_dead) return;
    _buildStates[s.appId] = s;
    if (_planSubs.containsKey(s.appId)) _changed();
  }

  void _onMakes() {
    _watchMakePlans();
    _changed();
  }

  void _onIcons() => _changed();

  /// Watches the plan of each make that keeps one, and stops watching the
  /// others.
  void _watchMakePlans() {
    final b = builds;
    if (b == null) return;
    final keys = {
      for (final m in makes.makes) ?m.planKey,
    };
    for (final gone
        in _makePlanSubs.keys.where((k) => !keys.contains(k)).toList()) {
      unawaited(_makePlanSubs.remove(gone)!.cancel());
      _makePlans.remove(gone);
    }
    for (final key in keys) {
      _makePlanSubs[key] ??= b.plans.watch(key).listen((p) {
        _makePlans[key] = p;
        _changed();
      }, onError: (Object _) {});
    }
  }

  @override
  void dispose() {
    _dead = true;
    makes.removeListener(_onMakes);
    icons?.removeListener(_onIcons);
    unawaited(_events?.cancel());
    unawaited(_builds?.cancel());
    for (final s in [..._planSubs.values, ..._makePlanSubs.values]) {
      unawaited(s.cancel());
    }
    unawaited(_out.close());
    if (closeStream) unawaited(stream.close());
    super.dispose();
  }
}
