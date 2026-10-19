import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../clax/api.dart' show ArtifactSummary;
import '../../home/home_host.dart';
import '../app_icon.dart';
import '../kit/kit.dart';
import '../theme.dart';
import '../tokens.g.dart';
import 'sidebar_model.dart';

/// How long typed letters add up to one type-select prefix.
const typeSelectPause = Duration(seconds: 1);

/// The Apple family's sidebar: a source list of [model]'s rows.
///
/// Rows are 28 high: a 20-point app icon (`tile` radius), the title in
/// `body` and the unread count as a trailing [ClaxBadge]; a make has its
/// ring and "3/7", and a second line while it is offline, waiting,
/// interrupted or failed. The selected row is filled with the system's
/// accent ([appleAccent]) while the sidebar has the keyboard focus, grey
/// without it; another row under the mouse is filled with `hover`, and the
/// mouse shows the arrow over every row. ↑ and ↓ move the
/// selection and typing selects by title ([typeSelectPause] apart starts a
/// new prefix, by [now]). An app dragged onto Starred or a place in Apps
/// is put there through `layout/main` ([SidebarModel.drop]); a secondary
/// click on an app calls [onMenu] with its row's rectangle. Group rows say
/// whether they are open to VoiceOver. A field at the bottom
/// ([filterFocus]) narrows the rows ([SidebarModel.filter]).
///
/// Its background is [background]: clear by default, for the window's
/// sidebar material behind the Flutter view to show.
class ClaxSidebar extends StatefulWidget {
  const ClaxSidebar({
    super.key,
    required this.model,
    this.onMenu,
    this.onSelected,
    this.filterFocus,
    this.background = Colors.transparent,
    this.now = DateTime.now,
  });

  final SidebarModel model;
  final void Function(String appId, Rect anchor)? onMenu;

  /// Called after a row was chosen with the pointer or the keyboard.
  final VoidCallback? onSelected;
  final FocusNode? filterFocus;
  final Color background;
  final DateTime Function() now;

  @override
  State<ClaxSidebar> createState() => _ClaxSidebarState();
}

class _ClaxSidebarState extends State<ClaxSidebar> {
  final _focus = FocusNode(debugLabel: 'sidebar');
  final _filter = TextEditingController();
  var _typed = '';
  DateTime? _typedAt;

  /// The row a drag is over, by its index in the rows.
  int? _over;

  @override
  void initState() {
    super.initState();
    _focus.addListener(_onFocus);
    _filter.text = widget.model.filter;
  }

  @override
  void dispose() {
    _focus
      ..removeListener(_onFocus)
      ..dispose();
    _filter.dispose();
    super.dispose();
  }

  void _onFocus() => setState(() {});

  void _choose(SidebarSelection s) {
    widget.model.select(s);
    _focus.requestFocus();
    widget.onSelected?.call();
  }

  KeyEventResult _onKey(FocusNode node, KeyEvent e) {
    if (e is! KeyDownEvent && e is! KeyRepeatEvent) return KeyEventResult.ignored;
    final k = e.logicalKey;
    if (k == LogicalKeyboardKey.arrowDown || k == LogicalKeyboardKey.arrowUp) {
      widget.model.moveSelection(k == LogicalKeyboardKey.arrowDown ? 1 : -1);
      return KeyEventResult.handled;
    }
    final keys = HardwareKeyboard.instance;
    final ch = e.character;
    if (ch == null ||
        ch.trim().isEmpty ||
        keys.isMetaPressed ||
        keys.isControlPressed ||
        keys.isAltPressed) {
      return KeyEventResult.ignored;
    }
    final now = widget.now();
    final last = _typedAt;
    if (last == null || now.difference(last) > typeSelectPause) _typed = '';
    _typed += ch;
    _typedAt = now;
    widget.model.typeSelect(_typed);
    return KeyEventResult.handled;
  }

  @override
  Widget build(BuildContext context) {
    final p = claxPalette(context);
    final top = MediaQuery.paddingOf(context).top;
    return Material(
      type: MaterialType.transparency,
      child: ColoredBox(
      color: widget.background,
      child: ListenableBuilder(
        listenable: widget.model,
        builder: (context, _) {
          final rows = widget.model.rows;
          final places = _places(rows);
          return Column(
            children: [
              Expanded(
                child: Focus(
                  focusNode: _focus,
                  onKeyEvent: _onKey,
                  child: ListView.builder(
                    key: const ValueKey('sidebar-list'),
                    padding: EdgeInsets.fromLTRB(
                      ClaxSpace.s8,
                      top + ClaxSpace.s4,
                      ClaxSpace.s8,
                      ClaxSpace.s8,
                    ),
                    itemCount: rows.length,
                    itemBuilder: (context, i) =>
                        _target(context, i, rows[i], places[i]),
                  ),
                ),
              ),
              Padding(
                padding: const EdgeInsets.fromLTRB(
                  ClaxSpace.s8,
                  ClaxSpace.s4,
                  ClaxSpace.s8,
                  ClaxSpace.s8,
                ),
                child: _FilterField(
                  controller: _filter,
                  focusNode: widget.filterFocus,
                  onChanged: (t) => widget.model.filter = t,
                  border: p.border,
                ),
              ),
            ],
          );
        },
      ),
      ),
    );
  }

  /// Where a drop on each row puts an app: (Starred, index) or (Apps,
  /// index among Apps' entries); null where nothing can be dropped.
  List<(bool, int)?> _places(List<SidebarRow> rows) {
    var star = 0;
    var entry = 0;
    return [
      for (final r in rows)
        switch (r) {
          SectionRow(section: SidebarSection.starred) => (true, 0),
          SectionRow(section: SidebarSection.apps) => (false, 0),
          AppRow(section: SidebarSection.starred) => (true, star++),
          AppRow(group: final g?) when g.isNotEmpty => (false, entry - 1),
          AppRow() || GroupRow() => (false, entry++),
          _ => null,
        },
    ];
  }

  Widget _target(BuildContext context, int i, SidebarRow row, (bool, int)? at) {
    final child = _row(context, row, dropOver: _over == i);
    if (at == null || !widget.model.canArrange) return child;
    final (toStarred, index) = at;
    return DragTarget<String>(
      onWillAcceptWithDetails: (d) {
        setState(() => _over = i);
        return true;
      },
      onLeave: (_) => setState(() => _over = null),
      onAcceptWithDetails: (d) {
        setState(() => _over = null);
        unawaited(
          widget.model.drop(d.data, index: index, toStarred: toStarred),
        );
      },
      builder: (context, _, _) => child,
    );
  }

  Widget _row(BuildContext context, SidebarRow row, {bool dropOver = false}) {
    final p = claxPalette(context);
    final platform = Theme.of(context).platform;
    final body = ClaxType.of(ClaxRole.body, platform);
    final model = widget.model;
    switch (row) {
      case SectionRow(:final title):
        return Container(
          height: 28,
          alignment: Alignment.bottomLeft,
          padding: const EdgeInsets.only(left: ClaxSpace.s8, bottom: 4),
          decoration: _dropLine(context, dropOver),
          child: Semantics(
            header: true,
            child: Text(
              title,
              style: ClaxType.of(
                ClaxRole.caption,
                platform,
              ).copyWith(color: p.muted, fontWeight: FontWeight.w600),
            ),
          ),
        );
      case GroupRow(:final group, :final expanded, :final count):
        return Semantics(
          button: true,
          expanded: expanded,
          label: '${group.name}, $count apps',
          excludeSemantics: true,
          onTap: () => model.toggleGroup(group.id),
          child: _SidebarTile(
            key: ValueKey('sidebar-group-${group.id}'),
            selected: false,
            focused: false,
            dropOver: dropOver,
            onTap: () => model.toggleGroup(group.id),
            leading: Icon(Icons.folder_outlined, size: 18, color: p.muted),
            title: Text(group.name, style: body.copyWith(color: p.fg)),
            trailing: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(
                  '$count',
                  style: ClaxType.of(
                    ClaxRole.caption,
                    platform,
                  ).copyWith(color: p.muted),
                ),
                const SizedBox(width: 2),
                AnimatedRotation(
                  turns: expanded ? 0.25 : 0,
                  duration: ClaxMotion.fast,
                  child: Icon(Icons.chevron_right, size: 14, color: p.muted),
                ),
              ],
            ),
          ),
        );
      case HomeRow():
        final selected = model.selection is HomeSelected;
        return _selectable(
          context,
          key: const ValueKey('sidebar-home'),
          selected: selected,
          label: 'Home',
          onTap: () => _choose(const HomeSelected()),
          leading: Icon(
            Icons.home_outlined,
            size: 18,
            color: _ink(context, selected, p.muted),
          ),
          title: 'Home',
        );
      case AppRow(:final app, :final unread, :final group):
        final selected = model.selection == AppSelected(app.id);
        final tile = _selectable(
          context,
          key: ValueKey('sidebar-app-${app.id}'),
          selected: selected,
          dropOver: dropOver,
          indent: group == null ? 0 : 20,
          label: app.title,
          onTap: () => _choose(AppSelected(app.id)),
          onSecondaryTap: widget.onMenu == null
              ? null
              : (rect) => widget.onMenu!(app.id, rect),
          leading: SidebarAppIcon(app: app, host: model.host),
          title: app.title,
          trailing: unread > 0 ? ClaxBadge(count: unread) : null,
        );
        if (!model.canArrange) return tile;
        return Draggable<String>(
          data: app.id,
          dragAnchorStrategy: pointerDragAnchorStrategy,
          feedback: Material(
            color: Colors.transparent,
            child: Opacity(
              opacity: 0.85,
              child: SizedBox(
                width: 200,
                child: _SidebarTile(
                  selected: false,
                  focused: false,
                  leading: SidebarAppIcon(app: app, host: model.host),
                  title: Text(
                    app.title,
                    style: body.copyWith(color: p.fg),
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
              ),
            ),
          ),
          childWhenDragging: Opacity(opacity: 0.4, child: tile),
          child: tile,
        );
      case MakeRow(:final make):
        final selected = model.selection == MakeSelected(make.id);
        final second = makeStateLine(make.state);
        final progress = make.progress;
        return _selectable(
          context,
          key: ValueKey('sidebar-make-${make.id}'),
          selected: selected,
          label: make.request,
          height: second == null ? 28 : 40,
          onTap: () => _choose(MakeSelected(make.id)),
          leading: _Ring(
            value: progress == null || progress.total == 0
                ? 0
                : progress.done / progress.total,
            color: _ink(context, selected, Theme.of(context).colorScheme.primary),
            track: _ink(context, selected, p.muted).withValues(alpha: 0.35),
          ),
          title: make.request,
          subtitle: second,
          trailing: progress == null
              ? null
              : Text(
                  '${progress.done}/${progress.total}',
                  style: ClaxType.of(
                    ClaxRole.caption,
                    platform,
                  ).copyWith(color: _ink(context, selected, p.muted)),
                ),
        );
    }
  }

  /// [fallback], or the accent's ink on a selected row that has the focus.
  Color _ink(BuildContext context, bool selected, Color fallback) =>
      selected && _focus.hasFocus ? appleOnAccent(context) : fallback;

  Widget _selectable(
    BuildContext context, {
    required Key key,
    required bool selected,
    required String label,
    required VoidCallback onTap,
    required Widget leading,
    required String title,
    String? subtitle,
    Widget? trailing,
    void Function(Rect)? onSecondaryTap,
    double indent = 0,
    double height = 28,
    bool dropOver = false,
  }) {
    final p = claxPalette(context);
    final platform = Theme.of(context).platform;
    final ink = _ink(context, selected, p.fg);
    return Semantics(
      key: key,
      button: true,
      selected: selected,
      label: label,
      excludeSemantics: true,
      onTap: onTap,
      child: _SidebarTile(
        selected: selected,
        focused: _focus.hasFocus,
        dropOver: dropOver,
        height: height,
        indent: indent,
        onTap: onTap,
        onSecondaryTap: onSecondaryTap,
        leading: leading,
        title: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              title,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: ClaxType.of(ClaxRole.body, platform).copyWith(color: ink),
            ),
            if (subtitle != null)
              Text(
                subtitle,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: ClaxType.of(
                  ClaxRole.caption,
                  platform,
                ).copyWith(color: _ink(context, selected, p.muted)),
              ),
          ],
        ),
        trailing: trailing,
      ),
    );
  }
}

/// The line under a make's request for its [state] (§4.2); none while it
/// is making.
String? makeStateLine(String state) => switch (state) {
  'offline' => 'Offline',
  'waiting' => 'Waiting',
  'interrupted' => 'Interrupted',
  'failed' => 'Couldn’t make it',
  _ => null,
};

/// A drop's place: an accent line along the row's top.
BoxDecoration? _dropLine(BuildContext context, bool on) => on
    ? BoxDecoration(
        border: Border(
          top: BorderSide(
            color: Theme.of(context).colorScheme.primary,
            width: 2,
          ),
        ),
      )
    : null;

class _SidebarTile extends StatefulWidget {
  const _SidebarTile({
    super.key,
    required this.selected,
    required this.focused,
    required this.leading,
    required this.title,
    this.trailing,
    this.onTap,
    this.onSecondaryTap,
    this.height = 28,
    this.indent = 0,
    this.dropOver = false,
  });

  final bool selected, focused, dropOver;
  final Widget leading, title;
  final Widget? trailing;
  final VoidCallback? onTap;
  final void Function(Rect)? onSecondaryTap;
  final double height, indent;

  @override
  State<_SidebarTile> createState() => _SidebarTileState();
}

class _SidebarTileState extends State<_SidebarTile> {
  var _hovered = false;

  @override
  Widget build(BuildContext context) {
    final p = claxPalette(context);
    final w = widget;
    final fill = w.selected
        ? (w.focused ? appleAccent(context) : p.fg.withValues(alpha: 0.1))
        : _hovered && w.onTap != null
        ? p.hover
        : null;
    return MouseRegion(
      cursor: SystemMouseCursors.basic,
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: w.onTap,
        onSecondaryTapUp: w.onSecondaryTap == null
            ? null
            : (_) {
                final box = context.findRenderObject() as RenderBox?;
                if (box == null) return;
                w.onSecondaryTap!(box.localToGlobal(Offset.zero) & box.size);
              },
        child: Container(
          height: w.height,
          decoration: _dropLine(context, w.dropOver),
          child: Container(
            key: const ValueKey('sidebar-row-fill'),
            margin: const EdgeInsets.symmetric(vertical: 1),
            padding: EdgeInsets.only(left: ClaxSpace.s8 + w.indent, right: 6),
            decoration: BoxDecoration(
              color: fill,
              borderRadius: BorderRadius.circular(ClaxRadius.controlApple),
            ),
            child: Row(
              children: [
                SizedBox(width: 20, child: Center(child: w.leading)),
                const SizedBox(width: 6),
                Expanded(child: w.title),
                ?w.trailing,
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// App [app]'s icon, [size] across (`tile` radius), from [host]'s list.
class SidebarAppIcon extends StatelessWidget {
  const SidebarAppIcon({
    super.key,
    required this.app,
    required this.host,
    this.size = 20,
  });

  final HomeApp app;
  final HomeHost host;
  final double size;

  @override
  Widget build(BuildContext context) {
    final summary =
        host.summaries?.where((a) => a.id == app.id).firstOrNull ??
        ArtifactSummary(
          id: app.id,
          title: app.title,
          currentVersion: 1,
          updatedAt: app.updatedAt.toIso8601String(),
        );
    return AppBadge(
      app: summary,
      svg: host.icons?.svgOf(app.id),
      size: size,
      radius: ClaxRadius.tile(size),
    );
  }
}

/// A make's progress ring, [value] from 0 to 1.
class _Ring extends StatelessWidget {
  const _Ring({required this.value, required this.color, required this.track});

  final double value;
  final Color color, track;

  @override
  Widget build(BuildContext context) => SizedBox(
    width: 14,
    height: 14,
    child: CircularProgressIndicator(
      value: value,
      strokeWidth: 2,
      color: color,
      backgroundColor: track,
    ),
  );
}

/// The sidebar's filter field.
class _FilterField extends StatelessWidget {
  const _FilterField({
    required this.controller,
    required this.onChanged,
    required this.border,
    this.focusNode,
  });

  final TextEditingController controller;
  final ValueChanged<String> onChanged;
  final FocusNode? focusNode;
  final Color border;

  @override
  Widget build(BuildContext context) {
    final p = claxPalette(context);
    final platform = Theme.of(context).platform;
    final style = ClaxType.of(ClaxRole.callout, platform);
    return SizedBox(
      height: 26,
      child: TextField(
        key: const ValueKey('sidebar-filter'),
        controller: controller,
        focusNode: focusNode,
        onChanged: onChanged,
        style: style.copyWith(color: p.fg),
        decoration: InputDecoration(
          isDense: true,
          hintText: 'Filter',
          hintStyle: style.copyWith(color: p.muted),
          prefixIcon: Icon(Icons.search, size: 14, color: p.muted),
          prefixIconConstraints: const BoxConstraints(minWidth: 24),
          contentPadding: const EdgeInsets.symmetric(vertical: 6),
          filled: true,
          fillColor: p.fg.withValues(alpha: 0.05),
          border: OutlineInputBorder(
            borderRadius: BorderRadius.circular(ClaxRadius.controlApple),
            borderSide: BorderSide(color: border),
          ),
          enabledBorder: OutlineInputBorder(
            borderRadius: BorderRadius.circular(ClaxRadius.controlApple),
            borderSide: BorderSide(color: border),
          ),
        ),
      ),
    );
  }
}
