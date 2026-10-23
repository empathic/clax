import 'package:flutter/material.dart';

import '../kit/kit.dart';
import '../theme.dart';
import '../tokens.g.dart';
import 'sidebar.dart' show SidebarAppIcon, makeStateLine;
import 'sidebar_model.dart';

/// The Material family's list pane beside the detail, [ClaxLayout.listPane]
/// wide, over the same [model] as the Mac's sidebar (§3.2): Search at the
/// top, then Starred, Apps (a group as a row that opens in place, its apps
/// indented under it) and Making. Home is the navigation rail's, so the
/// pane does not list it.
///
/// Rows are Material 3 list items 56 high: the app's 32 dp icon, its
/// title and the unread count as a trailing [ClaxBadge]; the selected row is
/// filled with the secondary container (`accent-tint`) as a stadium. A
/// make has its ring, "3/7" and a second line while it is offline,
/// waiting, interrupted or failed.
///
/// The search narrows the rows ([SidebarModel.filter]) and gives the
/// window's search to the home app's grid ([HomeHost.setQuery], heard
/// while the shell holds its toolbar); its More offers Import app…
/// ([HomeActions.importApp]) and, where offered, Get my apps from
/// clax.dev ([HomeActions.getApps]), which the home screen's own bar
/// carries where it has one. [searchFocus] is its keyboard
/// focus. [onSelected] runs after a row is chosen. A long press on an
/// app, or a secondary click at the pointer, opens its menu ([onMenu]).
class ClaxListPane extends StatefulWidget {
  const ClaxListPane({
    super.key,
    required this.model,
    this.searchFocus,
    this.onSelected,
    this.onMenu,
  });

  final SidebarModel model;
  final FocusNode? searchFocus;
  final VoidCallback? onSelected;

  /// Opens app [appId]'s menu at [anchor] (global coordinates): the
  /// pointer for a secondary click, the row for a long press.
  final void Function(String appId, Rect anchor)? onMenu;

  @override
  State<ClaxListPane> createState() => _ClaxListPaneState();
}

class _ClaxListPaneState extends State<ClaxListPane> {
  late final _search = TextEditingController(text: widget.model.filter);

  @override
  void dispose() {
    _search.dispose();
    super.dispose();
  }

  void _searched(String text) {
    widget.model.filter = text;
    widget.model.host.setQuery(text);
  }

  void _choose(SidebarSelection s) {
    widget.model.select(s);
    widget.onSelected?.call();
  }

  @override
  Widget build(BuildContext context) {
    final p = claxPalette(context);
    final scheme = Theme.of(context).colorScheme;
    return Material(
      color: p.bg,
      child: Column(
        children: [
          Padding(
            padding: EdgeInsets.fromLTRB(
              ClaxSpace.s12,
              MediaQuery.paddingOf(context).top + ClaxSpace.s12,
              ClaxSpace.s12,
              ClaxSpace.s8,
            ),
            child: SearchBar(
              key: const ValueKey('list-pane-search'),
              controller: _search,
              focusNode: widget.searchFocus,
              hintText: 'Search apps',
              elevation: const WidgetStatePropertyAll(0),
              backgroundColor: WidgetStatePropertyAll(
                scheme.surfaceContainerHigh,
              ),
              constraints: const BoxConstraints(minHeight: 56),
              leading: Icon(Icons.search, color: p.muted),
              trailing: [
                ListenableBuilder(
                  listenable: _search,
                  builder: (context, _) => _search.text.isEmpty
                      ? const SizedBox.shrink()
                      : IconButton(
                          tooltip: 'Clear',
                          icon: const Icon(Icons.close),
                          onPressed: () {
                            _search.clear();
                            _searched('');
                          },
                        ),
                ),
                PopupMenuButton<String>(
                  key: const ValueKey('list-pane-more'),
                  tooltip: 'More',
                  onSelected: (v) => v == 'get'
                      ? widget.model.host.actions.getApps()
                      : widget.model.host.actions.importApp(),
                  itemBuilder: (_) => [
                    const PopupMenuItem(
                      value: 'import',
                      child: Text('Import app…'),
                    ),
                    if (widget.model.host.offersGetApps)
                      const PopupMenuItem(
                        value: 'get',
                        child: Text('Get my apps from clax.dev'),
                      ),
                  ],
                ),
              ],
              onChanged: _searched,
            ),
          ),
          Expanded(
            child: ListenableBuilder(
              listenable: widget.model,
              builder: (context, _) {
                final rows = [
                  for (final r in widget.model.rows)
                    if (r is! HomeRow) r,
                ];
                if (rows.isEmpty && widget.model.filter.trim().isNotEmpty) {
                  return Padding(
                    padding: const EdgeInsets.all(ClaxSpace.s24),
                    child: Text(
                      'No apps match “${widget.model.filter.trim()}”.',
                      textAlign: TextAlign.center,
                      style: Theme.of(
                        context,
                      ).textTheme.bodyMedium?.copyWith(color: p.muted),
                    ),
                  );
                }
                return ListView.builder(
                  key: const ValueKey('list-pane'),
                  padding: const EdgeInsets.fromLTRB(
                    ClaxSpace.s12,
                    0,
                    ClaxSpace.s12,
                    ClaxSpace.s16,
                  ),
                  itemCount: rows.length,
                  itemBuilder: (context, i) => _row(context, rows[i]),
                );
              },
            ),
          ),
        ],
      ),
    );
  }

  Widget _row(BuildContext context, SidebarRow row) {
    final p = claxPalette(context);
    final text = Theme.of(context).textTheme;
    final model = widget.model;
    switch (row) {
      case SectionRow(:final title):
        return Padding(
          padding: const EdgeInsets.fromLTRB(
            ClaxSpace.s16,
            ClaxSpace.s16,
            ClaxSpace.s16,
            ClaxSpace.s8,
          ),
          child: Semantics(
            header: true,
            child: Text(
              title,
              style: text.labelMedium?.copyWith(color: p.muted),
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
          child: _PaneTile(
            key: ValueKey('pane-group-${group.id}'),
            selected: false,
            onTap: () => model.toggleGroup(group.id),
            leading: Icon(
              expanded ? Icons.folder_open_outlined : Icons.folder_outlined,
              color: p.muted,
            ),
            title: group.name,
            trailing: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(
                  '$count',
                  style: text.labelMedium?.copyWith(color: p.muted),
                ),
                const SizedBox(width: ClaxSpace.s4),
                AnimatedRotation(
                  turns: expanded ? 0.5 : 0,
                  duration: ClaxMotion.fast,
                  child: Icon(Icons.expand_more, color: p.muted),
                ),
              ],
            ),
          ),
        );
      case AppRow(:final app, :final unread, :final group):
        final selected = model.selection == AppSelected(app.id);
        return Semantics(
          button: true,
          selected: selected,
          label: unread > 0 ? '${app.title}, $unread unread' : app.title,
          excludeSemantics: true,
          onTap: () => _choose(AppSelected(app.id)),
          child: _PaneTile(
            key: ValueKey('pane-app-${app.id}'),
            selected: selected,
            indent: group == null ? 0 : ClaxSpace.s24,
            onTap: () => _choose(AppSelected(app.id)),
            onMenu: switch (widget.onMenu) {
              final f? => (anchor) => f(app.id, anchor),
              null => null,
            },
            leading: SidebarAppIcon(app: app, host: model.host, size: 32),
            title: app.title,
            trailing: unread > 0 ? ClaxBadge(count: unread) : null,
          ),
        );
      case MakeRow(:final make):
        final selected = model.selection == MakeSelected(make.id);
        final progress = make.progress;
        final second = makeStateLine(make.state);
        return Semantics(
          button: true,
          selected: selected,
          label: make.request,
          excludeSemantics: true,
          onTap: () => _choose(MakeSelected(make.id)),
          child: _PaneTile(
            key: ValueKey('pane-make-${make.id}'),
            selected: selected,
            height: second == null ? 56 : 64,
            onTap: () => _choose(MakeSelected(make.id)),
            leading: SizedBox(
              width: 22,
              height: 22,
              child: CircularProgressIndicator(
                value: progress == null || progress.total == 0
                    ? 0
                    : progress.done / progress.total,
                strokeWidth: 3,
                backgroundColor: p.muted.withValues(alpha: 0.25),
              ),
            ),
            title: make.request,
            subtitle: second,
            trailing: progress == null
                ? null
                : Text(
                    '${progress.done}/${progress.total}',
                    style: text.labelMedium?.copyWith(color: p.muted),
                  ),
          ),
        );
      case HomeRow():
        return const SizedBox.shrink();
    }
  }
}

/// One row of the pane: a Material 3 list item, filled as a stadium in the
/// secondary container when [selected].
class _PaneTile extends StatelessWidget {
  const _PaneTile({
    super.key,
    required this.selected,
    required this.onTap,
    required this.leading,
    required this.title,
    this.subtitle,
    this.trailing,
    this.indent = 0,
    this.height = 56,
    this.onMenu,
  });

  final bool selected;
  final VoidCallback onTap;

  /// Opens the row's menu at a rect in global coordinates: the row's for
  /// a long press, the pointer's for a secondary click.
  final ValueChanged<Rect>? onMenu;
  final Widget leading;
  final String title;
  final String? subtitle;
  final Widget? trailing;
  final double indent, height;

  @override
  Widget build(BuildContext context) {
    final p = claxPalette(context);
    final scheme = Theme.of(context).colorScheme;
    final text = Theme.of(context).textTheme;
    final ink = selected ? scheme.onSecondaryContainer : p.fg;
    return SizedBox(
      height: height,
      child: Material(
        color: selected ? scheme.secondaryContainer : Colors.transparent,
        shape: const StadiumBorder(),
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: onTap,
          onLongPress: switch (onMenu) {
            final f? => () {
              final box = context.findRenderObject()! as RenderBox;
              f(box.localToGlobal(Offset.zero) & box.size);
            },
            null => null,
          },
          onSecondaryTapUp: switch (onMenu) {
            final f? => (d) => f(d.globalPosition & Size.zero),
            null => null,
          },
          child: Padding(
            padding: EdgeInsets.only(
              left: ClaxSpace.s16 + indent,
              right: ClaxSpace.s16,
            ),
            child: Row(
              children: [
                SizedBox(width: 32, child: Center(child: leading)),
                const SizedBox(width: ClaxSpace.s16),
                Expanded(
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: text.bodyLarge?.copyWith(
                          color: ink,
                          fontWeight: selected ? FontWeight.w500 : null,
                        ),
                      ),
                      if (subtitle case final s?)
                        Text(
                          s,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: text.bodyMedium?.copyWith(color: p.muted),
                        ),
                    ],
                  ),
                ),
                if (trailing case final t?) ...[
                  const SizedBox(width: ClaxSpace.s8),
                  t,
                ],
              ],
            ),
          ),
        ),
      ),
    );
  }
}
