import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../theme.dart';
import 'family.dart';

/// The system bars' style over a surface of colour [under]: both bars
/// transparent, so the page draws edge to edge, with light icons when
/// [under] is dark (luminance below 0.5) and dark icons otherwise.
SystemUiOverlayStyle statusStyleFor(Color under) {
  final dark = under.computeLuminance() < 0.5;
  final icons = dark ? Brightness.light : Brightness.dark;
  return SystemUiOverlayStyle(
    statusBarColor: Colors.transparent,
    statusBarIconBrightness: icons,
    // iOS reads the brightness of what is under the bar, not of its icons.
    statusBarBrightness: dark ? Brightness.dark : Brightness.light,
    systemNavigationBarColor: Colors.transparent,
    systemNavigationBarDividerColor: Colors.transparent,
    systemNavigationBarIconBrightness: icons,
    systemNavigationBarContrastEnforced: false,
    systemStatusBarContrastEnforced: false,
  );
}

/// A screen: its [title] (with an optional [subtitle] under it), [leading]
/// and [actions] in the bar, and its [body].
///
/// In the Material family it is a `Scaffold` with an app bar in the page's
/// colour, the status bar's icons chosen by [statusStyleFor]; with
/// [largeTitle] the title starts large over the body and collapses into
/// the bar as the body scrolls. In the Apple family where the window has
/// its own toolbar ([hasDesktopWindowChrome]), it is the content of the
/// window or pane alone: the toolbar carries the title and the actions.
/// On iOS it keeps a bar with the title centred.
class ClaxScaffold extends StatelessWidget {
  const ClaxScaffold({
    super.key,
    this.title,
    this.subtitle,
    this.actions = const [],
    this.leading,
    required this.body,
    this.floatingActionButton,
    this.largeTitle = false,
  });

  final String? title;
  final String? subtitle;
  final List<Widget> actions;
  final Widget? leading;
  final Widget body;
  final Widget? floatingActionButton;
  final bool largeTitle;

  @override
  Widget build(BuildContext context) {
    final p = claxPalette(context);
    final theme = Theme.of(context);
    final barColor = theme.appBarTheme.backgroundColor ?? p.bg;
    final style = statusStyleFor(barColor);
    final apple = ClaxFamily.of(context) == ClaxFamily.apple;
    final toolbarCarriesTitle = apple && hasDesktopWindowChrome;
    final hasBar =
        !toolbarCarriesTitle &&
        (title != null || leading != null || actions.isNotEmpty);

    Widget? titleWidget;
    if (title != null) {
      titleWidget = subtitle == null
          ? Text(title!, maxLines: 1, overflow: TextOverflow.ellipsis)
          : Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: apple
                  ? CrossAxisAlignment.center
                  : CrossAxisAlignment.start,
              children: [
                Text(title!, maxLines: 1, overflow: TextOverflow.ellipsis),
                Text(
                  subtitle!,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: theme.textTheme.bodySmall!.copyWith(color: p.muted),
                ),
              ],
            );
    }

    final Widget content;
    final PreferredSizeWidget? appBar;
    if (hasBar && largeTitle && subtitle == null) {
      appBar = null;
      content = NestedScrollView(
        headerSliverBuilder: (context, _) => [
          SliverAppBar.large(
            title: titleWidget,
            leading: leading,
            actions: [...actions, const SizedBox(width: 4)],
            systemOverlayStyle: style,
            backgroundColor: barColor,
            surfaceTintColor: Colors.transparent,
          ),
        ],
        body: body,
      );
    } else {
      appBar = hasBar
          ? AppBar(
              title: titleWidget,
              leading: leading,
              centerTitle: apple,
              actions: [...actions, const SizedBox(width: 4)],
              systemOverlayStyle: style,
              backgroundColor: barColor,
            )
          : null;
      content = body;
    }

    return AnnotatedRegion<SystemUiOverlayStyle>(
      value: style,
      child: Scaffold(
        backgroundColor: p.bg,
        appBar: appBar,
        body: content,
        floatingActionButton: floatingActionButton,
      ),
    );
  }
}
