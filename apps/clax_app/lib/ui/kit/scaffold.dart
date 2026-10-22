import 'dart:math';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../layout.dart';
import '../theme.dart';
import '../tokens.g.dart';
import 'family.dart';
import 'window_titlebar.dart';

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
/// and [actions] in the bar, and its [body], or [slivers] scrolling in one
/// `CustomScrollView`.
///
/// In the Material family it is a `Scaffold` with an app bar in the page's
/// colour, the status bar's icons chosen by [statusStyleFor]; with
/// [largeTitle] the title starts large over the body and collapses into
/// the bar as the body scrolls, and a 1 px `border` line shows under the
/// bar while the body is scrolled under it ([slivers] scroll in the same
/// view as the bar, so none of them starts under it). In the Apple family
/// where the window has its own toolbar ([hasDesktopWindowChrome]), the
/// toolbar carries the title and the actions once [toolbarCarriesTitle] is
/// set, and the scaffold is the content of the window or pane alone; until
/// then a header shows them, with Back when the route can be left. At the
/// top of a window whose content runs under its title bar
/// ([WindowTitlebar.of]) the header is the toolbar row, its items after
/// the window buttons and centred with them; elsewhere it is below the top
/// padding. Nothing is drawn in the window buttons' corner. On iOS it
/// keeps a bar with the title centred.
class ClaxScaffold extends StatelessWidget {
  const ClaxScaffold({
    super.key,
    this.title,
    this.subtitle,
    this.actions = const [],
    this.leading,
    this.body,
    this.slivers,
    this.floatingActionButton,
    this.largeTitle = false,
    this.contentWidth,
  }) : assert(
         (body == null) != (slivers == null),
         'a ClaxScaffold takes a body or slivers',
       );

  final String? title;
  final String? subtitle;
  final List<Widget> actions;
  final Widget? leading;
  final Widget? body;
  final List<Widget>? slivers;
  final Widget? floatingActionButton;
  final bool largeTitle;

  /// From medium width, [slivers] (with a large title, the title too) keep
  /// to a centred column at most this wide; null spans the window.
  final double? contentWidth;

  /// Whether the Mac window's toolbar shows each screen's title, Back and
  /// actions; set once it does. Until then a Mac screen draws them in a
  /// header of its own.
  static bool toolbarCarriesTitle = false;

  @override
  Widget build(BuildContext context) {
    final p = claxPalette(context);
    final theme = Theme.of(context);
    final barColor = theme.appBarTheme.backgroundColor ?? p.bg;
    final style = statusStyleFor(barColor);
    final apple = ClaxFamily.of(context) == ClaxFamily.apple;
    final macWindow = apple && hasDesktopWindowChrome;
    final wantsBar = title != null || leading != null || actions.isNotEmpty;
    final hasBar = !macWindow && wantsBar;
    final header = macWindow && !toolbarCarriesTitle && wantsBar;

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
    final body =
        this.body ??
        CustomScrollView(slivers: [for (final s in slivers!) _column(s)]);
    SliverAppBar large(bool scrolled) => SliverAppBar.large(
      title: titleWidget,
      leading: leading,
      actions: [...actions, const SizedBox(width: 4)],
      systemOverlayStyle: style,
      backgroundColor: barColor,
      surfaceTintColor: Colors.transparent,
      scrolledUnderElevation: 0,
      forceElevated: scrolled,
      bottom: const _ScrolledUnderLine(),
    );
    if (hasBar && largeTitle && subtitle == null) {
      appBar = null;
      content = switch (slivers) {
        final slivers? => CustomScrollView(
          slivers: [
            for (final sliver in [large(false), ...slivers]) _column(sliver),
          ],
        ),
        null => NestedScrollView(
          headerSliverBuilder: (context, scrolled) => [large(scrolled)],
          body: body,
        ),
      };
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
      final top = MediaQuery.paddingOf(context).top;
      final bar = WindowTitlebar.of(context);
      // The screen is at the top of a window, under its title bar.
      final row = macWindow && bar != null && bar.top > 0 && top >= bar.top
          ? bar
          : null;
      final below = MediaQuery.removePadding(
        context: context,
        removeTop: true,
        child: body,
      );
      content = header
          ? Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                _MacHeader(
                  title: titleWidget,
                  leading: leading,
                  actions: actions,
                  row: row,
                  top: row == null ? top : 0,
                ),
                Expanded(child: below),
              ],
            )
          : macWindow
          ? Padding(
              padding: EdgeInsets.only(top: top),
              child: below,
            )
          : body;
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

extension on ClaxScaffold {
  /// [sliver] in the centred column [contentWidth] asks for.
  Widget _column(Widget sliver) {
    final width = contentWidth;
    if (width == null) return sliver;
    return SliverLayoutBuilder(
      builder: (context, box) {
        final side = ClaxWidth.of(context) >= ClaxWidth.medium
            ? max(0.0, (box.crossAxisExtent - width) / 2)
            : 0.0;
        return SliverPadding(
          padding: EdgeInsets.symmetric(horizontal: side),
          sliver: sliver,
        );
      },
    );
  }
}

/// A 1 px line along the bottom of a large-title bar: `border` while
/// content is under the bar, clear otherwise.
class _ScrolledUnderLine extends StatelessWidget
    implements PreferredSizeWidget {
  const _ScrolledUnderLine();

  @override
  Size get preferredSize => const Size.fromHeight(1);

  @override
  Widget build(BuildContext context) {
    final under =
        context
            .dependOnInheritedWidgetOfExactType<FlexibleSpaceBarSettings>()
            ?.isScrolledUnder ??
        false;
    return ColoredBox(
      key: const Key('clax-scrolled-under-line'),
      color: under ? claxPalette(context).border : const Color(0x00000000),
      child: const SizedBox(height: 1, width: double.infinity),
    );
  }
}

/// The title, Back (or [leading]) and the actions over a Mac screen's
/// content, while the window's toolbar does not carry them: in the
/// toolbar [row] when the screen is at the top of a window under it, after
/// its window buttons, else [top] below the top.
class _MacHeader extends StatelessWidget {
  const _MacHeader({
    required this.title,
    required this.leading,
    required this.actions,
    required this.row,
    required this.top,
  });

  final Widget? title;
  final Widget? leading;
  final List<Widget> actions;
  final WindowTitlebar? row;
  final double top;

  @override
  Widget build(BuildContext context) {
    final p = claxPalette(context);
    final theme = Theme.of(context);
    final back =
        leading ??
        (ModalRoute.of(context)?.impliesAppBarDismissal ?? false
            ? const BackButton()
            : null);
    final t = title;
    return DecoratedBox(
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: p.border)),
      ),
      child: ConstrainedBox(
        constraints: BoxConstraints(minHeight: max(44, row?.top ?? 0) + top),
        child: Padding(
          padding: EdgeInsets.only(
            top: top,
            left: switch ((row, back)) {
              (final row?, null) => row.leading + ClaxSpace.s12,
              (final row?, _) => row.leading + ClaxSpace.s4,
              (null, null) => ClaxSpace.s16,
              (null, _) => ClaxSpace.s4,
            },
            right: ClaxSpace.s8,
          ),
          child: Row(
            children: [
              ?back,
              if (t != null)
                Expanded(
                  child: DefaultTextStyle(
                    style: ClaxType.of(
                      ClaxRole.headline,
                      theme.platform,
                    ).copyWith(color: p.fg),
                    child: t,
                  ),
                )
              else
                const Spacer(),
              ...actions,
            ],
          ),
        ),
      ),
    );
  }
}
