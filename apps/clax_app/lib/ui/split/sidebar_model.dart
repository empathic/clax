/// What a wide window's sidebar lists and which of it is selected: Home,
/// the starred apps, every app and group in the home app's arrangement
/// (`layout/main`, through [HomeLayout]) and the makes going on, from
/// [HomeHost]'s lists.
library;

import 'package:flutter/foundation.dart';

import '../../home/home_host.dart';
import '../../home/home_layout.dart';

/// What the detail beside the sidebar shows.
@immutable
sealed class SidebarSelection {
  const SidebarSelection();
}

/// The home grid.
final class HomeSelected extends SidebarSelection {
  const HomeSelected();

  @override
  bool operator ==(Object other) => other is HomeSelected;

  @override
  int get hashCode => (HomeSelected).hashCode;

  @override
  String toString() => 'HomeSelected()';
}

/// App [id].
final class AppSelected extends SidebarSelection {
  const AppSelected(this.id);
  final String id;

  @override
  bool operator ==(Object other) => other is AppSelected && other.id == id;

  @override
  int get hashCode => Object.hash(AppSelected, id);

  @override
  String toString() => 'AppSelected($id)';
}

/// Make [id] ([HomeMake.id]).
final class MakeSelected extends SidebarSelection {
  const MakeSelected(this.id);
  final String id;

  @override
  bool operator ==(Object other) => other is MakeSelected && other.id == id;

  @override
  int get hashCode => Object.hash(MakeSelected, id);

  @override
  String toString() => 'MakeSelected($id)';
}

/// The sidebar's sections.
enum SidebarSection {
  starred('Starred'),
  apps('Apps'),
  making('Making');

  const SidebarSection(this.title);
  final String title;
}

/// One row of the sidebar.
@immutable
sealed class SidebarRow {
  const SidebarRow();
}

/// The top row, which selects the home grid.
final class HomeRow extends SidebarRow {
  const HomeRow();
}

/// A section's head.
final class SectionRow extends SidebarRow {
  const SectionRow(this.section);
  final SidebarSection section;
  String get title => section.title;
}

/// [app] in [section], inside [group] (its ID) when it is one's, with
/// [unread] unread threads.
final class AppRow extends SidebarRow {
  const AppRow(this.app, {required this.section, this.group, this.unread = 0});
  final HomeApp app;
  final SidebarSection section;
  final String? group;
  final int unread;
}

/// [group], a disclosure row in Apps, open when [expanded]; [count] apps.
final class GroupRow extends SidebarRow {
  const GroupRow(this.group, {required this.expanded});
  final LayoutGroup group;
  final bool expanded;
  int get count => group.apps.length;
}

/// [make], in Making.
final class MakeRow extends SidebarRow {
  const MakeRow(this.make);
  final HomeMake make;
}

/// The sidebar of a wide window over [host]: its [rows] and the
/// [selection], which the detail beside it shows.
///
/// Apps are arranged as `layout/main` arranges them ([HomeHost.layout]);
/// without a home app, or before its arrangement loads, every app is
/// listed newest first under Apps. An app the arrangement does not place
/// yet goes first in Apps, newest first, and one no longer listed is left
/// out. Starred and Making show only when they have rows. Groups start
/// closed.
///
/// While [filter] has text, only the apps and makes whose titles contain
/// it (ignoring case) are listed, with the heads of their sections; a
/// group with a match shows open with only its matches, and Home is not
/// listed.
///
/// [selection] lives in [selected] (made here when not given), so whoever
/// lays the window out can follow it. A selected app no longer listed
/// gives way to Home.
class SidebarModel extends ChangeNotifier {
  SidebarModel({
    required this.host,
    ValueNotifier<SidebarSelection>? selected,
    this.unreadOf,
  }) : selected = selected ?? ValueNotifier(const HomeSelected()),
       _ownsSelected = selected == null {
    host.addListener(_onHost);
    _follow(host.layout);
    _onHost();
  }

  final HomeHost host;

  /// The selection.
  final ValueNotifier<SidebarSelection> selected;
  final bool _ownsSelected;

  /// The unread threads of app [id]; none when null.
  final int Function(String id)? unreadOf;

  HomeLayout? _layout;
  final _open = <String>{};
  String _filter = '';
  List<SidebarRow>? _rows;

  SidebarSelection get selection => selected.value;

  /// What narrows the rows; '' for none.
  String get filter => _filter;
  set filter(String text) {
    if (text == _filter) return;
    _filter = text;
    _changed();
  }

  /// The rows, top to bottom.
  List<SidebarRow> get rows => _rows ??= _build();

  /// The starred apps listed now, in the person's order, whatever
  /// [filter] holds (⌘1…⌘9 select the first nine).
  List<String> get starred => _sections().starred;

  /// Shows [s] in the detail; an app in a closed group opens the group.
  void select(SidebarSelection s) {
    if (s case AppSelected(:final id)) {
      final gid = _groupOf(id);
      if (gid != null && _open.add(gid)) _changed();
    }
    if (selected.value == s) return;
    selected.value = s;
    notifyListeners();
  }

  /// Opens group [gid] if it is closed, closes it otherwise.
  void toggleGroup(String gid) {
    if (!_open.remove(gid)) _open.add(gid);
    _changed();
  }

  /// Whether group [gid] is open.
  bool isOpen(String gid) => _open.contains(gid);

  /// Selects the first app or make, in the order listed (a closed group's
  /// apps counted where the group is), whose title starts with [prefix],
  /// ignoring case; whether there was one.
  bool typeSelect(String prefix) {
    final p = prefix.toLowerCase();
    if (p.isEmpty) return false;
    for (final r in _build(allOpen: true)) {
      final (title, s) = switch (r) {
        AppRow(:final app) => (app.title, AppSelected(app.id)),
        MakeRow(:final make) => (make.request, MakeSelected(make.id)),
        _ => (null, null),
      };
      if (title != null && s != null && title.toLowerCase().startsWith(p)) {
        select(s);
        return true;
      }
    }
    return false;
  }

  /// Moves the selection [delta] rows down (up when negative), over
  /// section heads and group rows, stopping at the first and last.
  void moveSelection(int delta) {
    final selectable = [
      for (final r in rows) ?selectionOf(r),
    ];
    if (selectable.isEmpty) return;
    final at = selectable.indexOf(selection);
    final next = at < 0
        ? (delta >= 0 ? 0 : selectable.length - 1)
        : (at + delta).clamp(0, selectable.length - 1);
    select(selectable[next]);
  }

  /// What selecting [row] shows; null for a row that selects nothing.
  static SidebarSelection? selectionOf(SidebarRow row) => switch (row) {
    HomeRow() => const HomeSelected(),
    AppRow(:final app) => AppSelected(app.id),
    MakeRow(:final make) => MakeSelected(make.id),
    SectionRow() || GroupRow() => null,
  };

  /// App [appId] dropped at [index]: among Starred when [toStarred], else
  /// among Apps' entries (a group counts as one). One pinned write of
  /// `layout/main`; nothing without a readable arrangement.
  Future<void> drop(
    String appId, {
    required int index,
    bool toStarred = false,
  }) async {
    final l = _layout;
    if (l == null || !l.readable) return;
    if (toStarred) {
      await l.starAt(appId, index);
    } else {
      await l.moveTo(appId, index);
    }
  }

  /// Whether a drop can be written: the arrangement was read.
  bool get canArrange => _layout?.readable ?? false;

  void _follow(HomeLayout? l) {
    if (identical(l, _layout)) return;
    _layout?.removeListener(_changed);
    _layout = l;
    l?.addListener(_changed);
  }

  void _onHost() {
    _follow(host.layout);
    _rows = null;
    final s = selection;
    final listed = host.summaries;
    if (listed != null) {
      final gone = switch (s) {
        AppSelected(:final id) => !host.state.apps.any((a) => a.id == id),
        MakeSelected(:final id) => !host.state.makes.any((m) => m.id == id),
        HomeSelected() => false,
      };
      if (gone) selected.value = const HomeSelected();
    }
    notifyListeners();
  }

  void _changed() {
    _rows = null;
    notifyListeners();
  }

  /// The group app [id] is in, if any.
  String? _groupOf(String id) {
    for (final g in _sections().groups.values) {
      if (g.apps.contains(id)) return g.id;
    }
    return null;
  }

  /// The arrangement fitted to the apps listed now.
  LayoutSections _sections() {
    final apps = host.state.apps;
    final listed = {for (final a in apps) a.id};
    final s = _layout?.sections;
    if (s == null) {
      return LayoutSections(
        starred: const [],
        order: [for (final a in apps) a.id],
        groups: const {},
      );
    }
    final placed = <String>{
      ...s.starred,
      for (final k in s.order)
        if (!k.startsWith('g:')) k,
      for (final g in s.groups.values) ...g.apps,
    };
    final groups = <String, LayoutGroup>{
      for (final MapEntry(key: gid, value: g) in s.groups.entries)
        gid: LayoutGroup(g.id, g.name, g.look, [
          for (final id in g.apps)
            if (listed.contains(id)) id,
        ]),
    };
    return LayoutSections(
      starred: [
        for (final id in s.starred)
          if (listed.contains(id)) id,
      ],
      order: [
        // Apps listed but not placed yet: newest first, as the list is.
        for (final a in apps)
          if (!placed.contains(a.id)) a.id,
        for (final k in s.order)
          if (k.startsWith('g:')
              ? (groups[k.substring(2)]?.apps.isNotEmpty ?? false)
              : listed.contains(k))
            k,
      ],
      groups: groups,
    );
  }

  List<SidebarRow> _build({bool allOpen = false}) {
    final state = host.state;
    final byId = {for (final a in state.apps) a.id: a};
    final s = _sections();
    final q = _filter.toLowerCase();
    bool hit(String title) => q.isEmpty || title.toLowerCase().contains(q);
    AppRow row(HomeApp a, SidebarSection section, [String? group]) => AppRow(
      a,
      section: section,
      group: group,
      unread: unreadOf?.call(a.id) ?? 0,
    );

    final starred = [
      for (final id in s.starred)
        if (byId[id] case final a? when hit(a.title))
          row(a, SidebarSection.starred),
    ];
    final apps = <SidebarRow>[];
    for (final k in s.order) {
      if (!k.startsWith('g:')) {
        if (byId[k] case final a? when hit(a.title)) {
          apps.add(row(a, SidebarSection.apps));
        }
        continue;
      }
      final g = s.groups[k.substring(2)];
      if (g == null) continue;
      final members = [
        for (final id in g.apps)
          if (byId[id] case final a? when q.isEmpty || hit(a.title) || hit(g.name))
            a,
      ];
      if (members.isEmpty) continue;
      final open = allOpen || q.isNotEmpty || _open.contains(g.id);
      apps.add(GroupRow(g, expanded: open));
      if (open) {
        for (final a in members) {
          apps.add(row(a, SidebarSection.apps, g.id));
        }
      }
    }
    final making = [
      for (final m in state.makes)
        if (hit(m.request)) MakeRow(m),
    ];
    return [
      if (q.isEmpty) const HomeRow(),
      if (starred.isNotEmpty) ...[
        const SectionRow(SidebarSection.starred),
        ...starred,
      ],
      if (q.isEmpty || apps.isNotEmpty) ...[
        const SectionRow(SidebarSection.apps),
        ...apps,
      ],
      if (making.isNotEmpty) ...[
        const SectionRow(SidebarSection.making),
        ...making,
      ],
    ];
  }

  @override
  void dispose() {
    host.removeListener(_onHost);
    _layout?.removeListener(_changed);
    if (_ownsSelected) selected.dispose();
    super.dispose();
  }
}
