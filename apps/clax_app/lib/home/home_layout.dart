/// The home screen's arrangement as the home app keeps it: the
/// `layout/main` document in the home app's own documents, read and
/// written as the owner.
///
/// The rules over the document are `apps/clax_app/home/arrange.js`'s
/// (`normalize`, `reconcile`, `moveTo`, `keepNewer`), so a window showing
/// the arrangement natively shows the same Starred, order and groups as
/// the home app's grid:
///
/// ```text
/// {v: 1, starred: [appId], order: [appId | "g:<groupId>"],
///  groups: {groupId: {name, look: "folder" | "pile", apps: [appId]}},
///  seen: [appId], opens: {appId: {n, last, days}}}
/// ```
///
/// `bigTiles` and `big`, which earlier releases wrote, are unknown fields
/// here: kept as they are and read by nothing.
///
/// Every function keeps what it does not know: unknown fields, at the top
/// and in a group or an open count; a group's unknown look; an order entry
/// of an unknown kind (a string `<kind>:…` other than `g:…`, or an
/// object), in place and not shown; and a `v` above 1, never lowered.
library;

import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';

import '../clax/api.dart';
import 'home_host.dart' show HomeApp;

/// The home app's arrangement document.
const layoutCollection = 'layout';
const layoutDocId = 'main';
const layoutPath = '$layoutCollection/$layoutDocId';

/// A layout document's body, as JSON.
typedef LayoutJson = Map<String, Object?>;

const _group = 'g:';
final _day = RegExp(r'^\d{4}-\d{2}-\d{2}$');
final _kind = RegExp(r'^[^:]+:');

bool _isGroupKey(Object? k) => k is String && k.startsWith(_group);
String _gidOf(String key) => key.substring(_group.length);
bool _isId(Object? v) => v is String && v.isNotEmpty && v.length <= 64;
LayoutJson? _obj(Object? v) => v is Map ? v.cast<String, Object?>() : null;

/// An order entry of a kind this release does not know: kept, not shown.
bool _isOther(Object? v) =>
    (v is String && _kind.hasMatch(v) && !_isGroupKey(v)) || _obj(v) != null;
bool _isKey(Object? v) =>
    _isOther(v) || _isId(v) || (_isGroupKey(v) && _isId(_gidOf(v as String)));
List<String> _ids(Object? v) => v is List
    ? [
        ...{for (final x in v) if (_isId(x)) x as String},
      ]
    : <String>[];
bool _isInt(Object? n) => n is int || (n is double && n == n.truncate());
bool _isFiniteNum(Object? n) => n is num && n.isFinite;

/// [body] (the stored document, or null) as a layout: every known field
/// there and well formed, every unknown field as it was.
LayoutJson normalizeLayout(Object? body) {
  final b = _obj(body) ?? const <String, Object?>{};
  final groups = <String, Object?>{};
  for (final MapEntry(key: gid, value: v) in (_obj(b['groups']) ?? {}).entries) {
    final g = _obj(v);
    if (!_isId(gid) || g == null) continue;
    final name = g['name'];
    final look = g['look'];
    groups[gid] = {
      ...g,
      'name': name is String && name.trim().isNotEmpty ? name : 'Group',
      'look': look is String && look.isNotEmpty ? look : 'folder',
      'apps': _ids(g['apps']),
    };
  }
  final opens = <String, Object?>{};
  for (final MapEntry(key: id, value: v) in (_obj(b['opens']) ?? {}).entries) {
    final o = _obj(v);
    if (!_isId(id) || o == null) continue;
    final days = <String, Object?>{
      for (final MapEntry(key: d, value: n) in (_obj(o['days']) ?? {}).entries)
        if (_day.hasMatch(d) && _isInt(n) && (n as num) > 0) d: n,
    };
    final n = o['n'];
    final last = o['last'];
    opens[id] = {
      ...o,
      'n': _isInt(n) && (n as num) > 0 ? n : 0,
      'last': last is String ? last : null,
      'days': days,
    };
  }
  final raw = b['order'];
  final order = raw is List
      ? [
          for (final (i, k) in raw.indexed)
            if (_isKey(k) && (k is! String || raw.indexOf(k) == i)) k,
        ]
      : <Object?>[];
  final v = b['v'];
  return {
    ...b,
    'v': _isFiniteNum(v) && (v as num) >= 1 ? v : 1,
    'starred': _ids(b['starred']),
    'order': order,
    'groups': groups,
    'seen': _ids(b['seen']),
    'opens': opens,
  };
}

/// Whether a stored known field's value is one this release reads.
final _reads = <String, bool Function(Object?)>{
  'starred': (v) => v is List,
  'order': (v) => v is List,
  'seen': (v) => v is List,
  'groups': (v) => _obj(v) != null,
  'opens': (v) => _obj(v) != null,
};

/// The body to write when [next] replaces [stored]: when [stored] is a
/// later release's layout (`v` above 1), its `v`, never lowered, and each
/// known field there whose value this release cannot read, as it was.
LayoutJson keepNewerLayout(Object? stored, LayoutJson next) {
  final s = _obj(stored);
  final sv = s?['v'];
  if (s == null || !(_isFiniteNum(sv) && (sv as num) > 1)) return next;
  final nv = next['v'];
  final out = <String, Object?>{
    ...next,
    'v': _isFiniteNum(nv) && (nv as num) > sv ? nv : sv,
  };
  for (final MapEntry(key: k, value: reads) in _reads.entries) {
    if (s.containsKey(k) && !reads(s[k])) out[k] = s[k];
  }
  return out;
}

/// [apps] newest first; apps updated at the same time keep their order.
List<HomeApp> _newestFirst(List<HomeApp> apps) {
  final indexed = apps.indexed.toList()
    ..sort((x, y) {
      final c = y.$2.updatedAt.compareTo(x.$2.updatedAt);
      return c != 0 ? c : x.$1.compareTo(y.$1);
    });
  return [for (final (_, a) in indexed) a];
}

/// [layout] made to match [apps]: an ID no longer listed goes from
/// everywhere; an app in two places keeps the first of Starred, its
/// group, All apps; a group left with one app gives way to that app in
/// its place, one left with none goes, and one no entry names goes last
/// in All apps; a listed app in no place goes first in All apps, newest
/// first. [first] (nothing stored yet) marks every listed app seen.
LayoutJson reconcileLayout(
  Object? layout,
  List<HomeApp> apps, {
  bool first = false,
}) {
  final l = normalizeLayout(layout);
  final listed = {for (final a in apps) a.id};
  final placed = <String>{};
  bool take(String id) => listed.contains(id) && placed.add(id);
  final starred = (l['starred'] as List<String>).where(take).toList();
  final groupsIn = (l['groups'] as Map).cast<String, LayoutJson>();
  final members = <String, List<String>>{
    for (final MapEntry(key: gid, value: g) in groupsIn.entries)
      gid: (g['apps'] as List<String>).where(take).toList(),
  };
  final order = <Object?>[];
  final named = <String>{};
  void placeGroup(String gid) {
    named.add(gid);
    final m = members[gid]!;
    if (m.length >= 2) {
      order.add('$_group$gid');
    } else if (m.length == 1) {
      order.add(m.single);
    }
  }

  for (final key in l['order'] as List) {
    if (_isOther(key)) {
      order.add(key);
    } else if (_isGroupKey(key)) {
      final gid = _gidOf(key as String);
      if (members.containsKey(gid) && !named.contains(gid)) placeGroup(gid);
    } else if (take(key as String)) {
      order.add(key);
    }
  }
  for (final gid in members.keys) {
    if (!named.contains(gid)) placeGroup(gid);
  }
  final groups = <String, Object?>{
    for (final MapEntry(key: gid, value: g) in groupsIn.entries)
      if (members[gid]!.length >= 2) gid: {...g, 'apps': members[gid]},
  };
  final fresh = [
    for (final a in _newestFirst(apps))
      if (!placed.contains(a.id)) a.id,
  ];
  final seenBefore = (l['seen'] as List<String>).where(listed.contains);
  final seen = first
      ? [
          ...{...seenBefore, for (final a in apps) a.id},
        ]
      : seenBefore.toList();
  final opens = <String, Object?>{
    for (final MapEntry(key: id, value: o) in (l['opens'] as Map).entries)
      if (listed.contains(id)) id as String: o,
  };
  return {
    ...l,
    'starred': starred,
    'order': [...fresh, ...order],
    'groups': groups,
    'seen': seen,
    'opens': opens,
  };
}

/// Where an entry goes: Starred, or All apps.
enum LayoutSection { starred, all }

/// [list] (an order) with [key] put before its [index]-th shown entry,
/// clamped (null is the end): an index counts only the entries shown.
List<Object?> _insertShown(List<Object?> list, Object? key, int? index) {
  final at = [
    for (final (i, k) in list.indexed)
      if (!_isOther(k)) i,
  ];
  final n = (index ?? at.length).clamp(0, at.length);
  final i = n < at.length ? at[n] : list.length;
  return [...list.sublist(0, i), key, ...list.sublist(i)];
}

/// [key] (an app ID or a group's `g:<gid>`) moved to [section] at [index]
/// among its entries, clamped (null is the end), out of wherever it was;
/// a group it leaves keeps its other apps. A group never goes to Starred:
/// that move changes nothing.
LayoutJson moveLayout(
  Object? layout,
  String key,
  LayoutSection section,
  int? index,
) {
  final l = normalizeLayout(layout);
  if (_isGroupKey(key) && section == LayoutSection.starred) return l;
  List<T> out<T>(List<T> list) => [
    for (final k in list)
      if (k != key) k,
  ];
  final groups = <String, Object?>{
    for (final MapEntry(key: gid, value: g)
        in (l['groups'] as Map).cast<String, LayoutJson>().entries)
      gid: {...g, 'apps': out(g['apps'] as List<String>)},
  };
  final starredOut = out(l['starred'] as List<String>);
  final starred = section == LayoutSection.starred
      ? (() {
          final i = (index ?? starredOut.length).clamp(0, starredOut.length);
          return [
            ...starredOut.sublist(0, i),
            key,
            ...starredOut.sublist(i),
          ];
        })()
      : starredOut;
  final orderOut = out(l['order'] as List<Object?>);
  final order = section == LayoutSection.all
      ? _insertShown(orderOut, key, index)
      : orderOut;
  return {...l, 'starred': starred, 'order': order, 'groups': groups};
}

/// One group under All apps: [id] without its `g:`, [look] `folder` or
/// `pile` (an unknown look shows as a folder), and its [apps] in order.
final class LayoutGroup {
  const LayoutGroup(this.id, this.name, this.look, this.apps);
  final String id, name, look;
  final List<String> apps;
}

/// What the arrangement shows: [starred]'s app IDs, then All apps'
/// entries in [order] (app IDs, and `g:<gid>` for a group in [groups]).
final class LayoutSections {
  const LayoutSections({
    required this.starred,
    required this.order,
    required this.groups,
  });

  final List<String> starred;

  /// App IDs and `g:<gid>`.
  final List<String> order;
  final Map<String, LayoutGroup> groups;
}

/// What a reconciled [layout] shows of [apps]: entries of unknown kinds,
/// unlisted apps and groups without apps left out.
LayoutSections layoutSections(LayoutJson layout, List<HomeApp> apps) {
  final listed = {for (final a in apps) a.id};
  final groupsIn = (layout['groups'] as Map).cast<String, LayoutJson>();
  final groups = <String, LayoutGroup>{};
  final order = <String>[];
  for (final key in layout['order'] as List) {
    if (_isOther(key)) continue;
    final k = key as String;
    if (!_isGroupKey(k)) {
      if (listed.contains(k)) order.add(k);
      continue;
    }
    final gid = _gidOf(k);
    final g = groupsIn[gid];
    final members = [
      for (final id in (g?['apps'] as List<String>?) ?? const <String>[])
        if (listed.contains(id)) id,
    ];
    if (g == null || members.isEmpty) continue;
    order.add(k);
    groups[gid] = LayoutGroup(
      gid,
      g['name'] as String,
      g['look'] == 'pile' ? 'pile' : 'folder',
      members,
    );
  }
  return LayoutSections(
    starred: [
      for (final id in layout['starred'] as List<String>)
        if (listed.contains(id)) id,
    ],
    order: order,
    groups: groups,
  );
}

LayoutSections _newestFirstSections(List<HomeApp> apps) => LayoutSections(
  starred: const [],
  order: [for (final a in _newestFirst(apps)) a.id],
  groups: const {},
);

/// Whether [data] is a layout this release reads: each of `starred`,
/// `order` and `groups` absent or of the type the arrangement keeps.
bool _readable(LayoutJson data) => [
  'starred',
  'order',
  'groups',
].every((k) => !data.containsKey(k) || _reads[k]!(data[k]));

/// The home app [homeAid]'s arrangement, read and written as the owner.
///
/// [load] reads `layout/main` and reconciles it with the apps listed;
/// [sections] is what it shows. A missing or unreadable document (one
/// whose `starred`, `order` or `groups` has another type, or a home app
/// that cannot be read) makes [readable] false: [sections] then lists the
/// apps newest first, and nothing is written.
///
/// [star], [unstar] and [moveTo] each write the whole document, pinned to
/// the version read: the read document with its `starred`, `order` and
/// `groups` changed and every other field (`v`, `seen`, `opens` and
/// anything later) as it was. A write refused as a
/// conflict reads the document again and makes the change again over it
/// once; a second conflict reads it again and rethrows. Loads and writes
/// run one at a time, in the order asked.
class HomeLayout extends ChangeNotifier {
  HomeLayout({required this.api, required this.homeAid});

  final ClaxApi api;
  final String homeAid;

  List<HomeApp> _apps = const [];
  Doc? _doc;
  LayoutSections _sections = _newestFirstSections(const []);
  Future<void> _queue = Future.value();
  var _dead = false;

  /// What the arrangement shows of the apps last loaded.
  LayoutSections get sections => _sections;

  /// Whether `layout/main` was read and is a layout this release reads.
  bool get readable => _doc != null;

  /// Reads the arrangement again, reconciled with [apps] (the apps
  /// listed, the home app left out).
  Future<void> load(List<HomeApp> apps) {
    _apps = List.of(apps);
    return _enqueue(_read);
  }

  /// App [id] last in Starred, out of any group.
  Future<void> star(String id) =>
      _change((l) => moveLayout(l, id, LayoutSection.starred, null));

  /// App [id] to [index] among Starred, clamped, out of any group and out
  /// of All apps.
  Future<void> starAt(String id, int index) =>
      _change((l) => moveLayout(l, id, LayoutSection.starred, index));

  /// App [id] first in All apps.
  Future<void> unstar(String id) =>
      _change((l) => moveLayout(l, id, LayoutSection.all, 0));

  /// [id] (an app ID or `g:<gid>`) to [index] among All apps' shown
  /// entries, clamped.
  Future<void> moveTo(String id, int index) =>
      _change((l) => moveLayout(l, id, LayoutSection.all, index));

  Future<void> _enqueue(Future<void> Function() run) {
    final next = _queue.then((_) => run());
    _queue = next.catchError((Object _) {});
    return next;
  }

  /// The stored [data] reconciled with the apps, or only normalized while
  /// no app is listed, so a list that is briefly empty never drops an ID.
  LayoutJson _fit(LayoutJson data) =>
      _apps.isEmpty ? normalizeLayout(data) : reconcileLayout(data, _apps);

  Future<void> _read() async {
    Doc? doc;
    try {
      doc = await api.getDoc(homeAid, layoutPath);
    } catch (_) {
      doc = null;
    }
    if (_dead) return;
    _doc = doc != null && _readable(doc.data) ? doc : null;
    _sections = switch (_doc) {
      final d? => layoutSections(_fit(d.data), _apps),
      null => _newestFirstSections(_apps),
    };
    notifyListeners();
  }

  Future<void> _change(LayoutJson Function(LayoutJson) op) => _enqueue(() async {
    for (var tries = 0; ; tries++) {
      final doc = _doc;
      if (doc == null || _dead) return;
      final changed = op(_fit(doc.data));
      final next = keepNewerLayout(doc.data, {
        ...doc.data,
        'starred': changed['starred'],
        'order': changed['order'],
        'groups': changed['groups'],
      });
      if (jsonEncode(next) == jsonEncode(doc.data)) return;
      try {
        await _write(next, doc.version);
      } on ClaxApiError catch (e) {
        await _read();
        if (!e.isConflict || tries >= 1) rethrow;
        continue;
      }
      await _read();
      return;
    }
  });

  Future<void> _write(LayoutJson next, int version) => api.batchDocs(homeAid, [
    DocWrite(
      op: 'set',
      collection: layoutCollection,
      docId: layoutDocId,
      data: next,
      ifVersion: version,
    ),
  ]);

  @override
  void dispose() {
    _dead = true;
    super.dispose();
  }
}
