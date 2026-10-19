import 'package:flutter/material.dart';

import '../theme.dart';
import '../tokens.g.dart';
import 'sidebar.dart' show makeStateLine;
import 'sidebar_model.dart';

/// The Material family's list pane beside the detail, [ClaxLayout.listPane]
/// wide: [model]'s rows as a plain list, a row tapped selecting what it
/// shows. Its Material form (Search, the rail beside it, M3 list items)
/// is not drawn yet.
class ClaxListPane extends StatelessWidget {
  const ClaxListPane({super.key, required this.model});

  final SidebarModel model;

  @override
  Widget build(BuildContext context) {
    final p = claxPalette(context);
    return Material(
      color: p.bg,
      child: ListenableBuilder(
        listenable: model,
        builder: (context, _) => ListView(
          key: const ValueKey('list-pane'),
          padding: EdgeInsets.only(
            top: MediaQuery.paddingOf(context).top + ClaxSpace.s8,
          ),
          children: [
            for (final r in model.rows)
              switch (r) {
                SectionRow(:final title) => Padding(
                  padding: const EdgeInsets.fromLTRB(16, 16, 16, 4),
                  child: Text(
                    title,
                    style: Theme.of(context).textTheme.labelMedium,
                  ),
                ),
                GroupRow(:final group, :final expanded, :final count) =>
                  ListTile(
                    leading: Icon(
                      expanded ? Icons.folder_open_outlined : Icons.folder_outlined,
                    ),
                    title: Text(group.name),
                    trailing: Text('$count'),
                    onTap: () => model.toggleGroup(group.id),
                  ),
                _ => ListTile(
                  selected: SidebarModel.selectionOf(r) == model.selection,
                  contentPadding: EdgeInsets.only(
                    left: r is AppRow && r.group != null ? 32 : 16,
                    right: 16,
                  ),
                  leading: Icon(switch (r) {
                    HomeRow() => Icons.home_outlined,
                    MakeRow() => Icons.hourglass_empty,
                    _ => Icons.apps,
                  }),
                  title: Text(switch (r) {
                    HomeRow() => 'Home',
                    AppRow(:final app) => app.title,
                    MakeRow(:final make) => make.request,
                    _ => '',
                  }),
                  subtitle: switch (r) {
                    MakeRow(:final make) => switch (makeStateLine(make.state)) {
                      final s? => Text(s),
                      null => null,
                    },
                    _ => null,
                  },
                  onTap: () => model.select(SidebarModel.selectionOf(r)!),
                ),
              },
          ],
        ),
      ),
    );
  }
}
