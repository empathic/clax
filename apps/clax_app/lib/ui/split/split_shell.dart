import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../home/home_host.dart';
import '../kit/kit.dart';
import '../layout.dart';
import '../mac/mac_toolbar.dart';
import '../mac/window_channel.dart';
import '../theme.dart';
import '../tokens.g.dart';
import 'list_pane.dart';
import 'sidebar.dart';
import 'sidebar_model.dart';

/// A wide window's layout: a sidebar of [model]'s rows and, beside it, the
/// detail of what is selected: [home] (the home grid) with Home selected,
/// [detailFor] of the selection otherwise. [home] stays built while
/// another detail shows, offstage, so its page is kept.
///
/// Laid out by the window's size class, never by platform:
/// - At [ClaxWidth.expanded] and up the sidebar stays beside the detail.
///   The Apple family's is [ClaxSidebar], [sidebarWidth] wide, which its
///   edge resizes between [ClaxLayout.sidebarMin] and
///   [ClaxLayout.sidebarMax] ([onSidebarWidth] hears the width a resize
///   ends at); the Material family's is [ClaxListPane].
/// - At [ClaxWidth.medium] the Apple family shows only the detail; the
///   sidebar comes over it when toggled, and goes once a row is chosen.
///
/// [toggleSidebar] hides and shows it, as ⌃⌘S (Control-S on Material)
/// does. ⌘F (Control-F) shows it and puts the keyboard in its filter.
///
/// Where the window has Mac chrome ([hasDesktopWindowChrome]) the
/// sidebar is clear over the window's sidebar material
/// ([MacWindow.setSidebarWidth]), the toolbar leads with the sidebar's
/// toggle ([MacToolbar.setLeading]) and the window's title follows the
/// selection: the app's title, the make's request, "Clax" for Home. The
/// detail's title bar ([WindowTitlebar]) is the window's, less what the
/// sidebar covers of it. There the shell also holds the window's toolbar
/// for the home app ([HomeHost.holdToolbar]) while it is built, so the
/// home app leaves its top bar, search and New app to the sidebar, the
/// toolbar and the menu bar.
class SplitShell extends StatefulWidget {
  const SplitShell({
    super.key,
    required this.model,
    required this.home,
    required this.detailFor,
    this.sidebarWidth = ClaxLayout.sidebar,
    this.onSidebarWidth,
    this.onMenu,
    this.onSidebarShown,
    this.now = DateTime.now,
  });

  final SidebarModel model;
  final Widget home;
  final Widget Function(SidebarSelection selection) detailFor;
  final double sidebarWidth;
  final ValueChanged<double>? onSidebarWidth;

  /// A secondary click on app [appId]'s row, at [anchor].
  final void Function(String appId, Rect anchor)? onMenu;

  /// Told whether the sidebar shows, after each frame in which that
  /// changed (for View › Show Sidebar or Hide Sidebar).
  final ValueChanged<bool>? onSidebarShown;

  /// The clock type-select reads.
  final DateTime Function() now;

  /// Whether the window at [context] lays out as a split: the Apple
  /// family from [ClaxWidth.medium], the Material family from
  /// [ClaxWidth.expanded] (its medium form, a rail and one pane, is not
  /// built yet).
  static bool fits(BuildContext context) =>
      ClaxWidth.of(context) >=
      (ClaxFamily.of(context) == ClaxFamily.apple
          ? ClaxWidth.medium
          : ClaxWidth.expanded);

  @override
  State<SplitShell> createState() => SplitShellState();
}

class SplitShellState extends State<SplitShell> {
  final _filterFocus = FocusNode(debugLabel: 'sidebar filter');
  late double _width = _clamp(widget.sidebarWidth);
  StreamSubscription<ToolbarClick>? _clicks;

  /// Whether the person hid the beside-the-detail sidebar, or brought
  /// the over-the-detail one up; each is reset when the size class
  /// changes.
  var _hidden = false;
  var _over = false;
  bool? _wasWide;
  int? _view;
  double? _sentWidth;
  double? _sentEdge;
  bool? _sentShown;

  static double _clamp(double w) =>
      w.clamp(ClaxLayout.sidebarMin, ClaxLayout.sidebarMax).toDouble();

  bool get _wide =>
      ClaxWidth.of(context) >= ClaxWidth.expanded;

  /// Whether the sidebar shows, beside or over the detail.
  bool get sidebarShown => _wasWide ?? true ? !_hidden : _over;

  /// Hides the sidebar if it shows, shows it otherwise.
  void toggleSidebar() => setState(() {
    if (_wide) {
      _hidden = !_hidden;
    } else {
      _over = !_over;
    }
  });

  @override
  void initState() {
    super.initState();
    widget.model.selected.addListener(_onSelected);
    if (hasDesktopWindowChrome) {
      _clicks = MacToolbar.clicks.listen((c) {
        if (mounted && c.view == _view && c.id == 'sidebar') toggleSidebar();
      });
    }
  }

  @override
  void didUpdateWidget(SplitShell oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.model != widget.model) {
      if (oldWidget.model.host != widget.model.host) {
        _release(oldWidget.model.host);
      }
      oldWidget.model.selected.removeListener(_onSelected);
      widget.model.selected.addListener(_onSelected);
    }
    if (oldWidget.sidebarWidth != widget.sidebarWidth) {
      _width = _clamp(widget.sidebarWidth);
    }
  }

  /// Lets [host] go of the window's toolbar, after the frame being torn
  /// down: the home host's listeners rebuild.
  void _release(HomeHost host) =>
      scheduleMicrotask(() => host.holdToolbar(this, false));

  @override
  void dispose() {
    _release(widget.model.host);
    widget.model.selected.removeListener(_onSelected);
    unawaited(_clicks?.cancel());
    if (_view case final view?) {
      MacToolbar.setLeading(const [], view: view);
      unawaited(MacWindow.setSidebarWidth(0, view: view));
    }
    _filterFocus.dispose();
    super.dispose();
  }

  void _onSelected() {
    if (!mounted) return;
    setState(() {});
    // After the frame, so a detail going away has said its last.
    WidgetsBinding.instance.addPostFrameCallback((_) => _sendTitle());
  }

  void _sendTitle() {
    final view = _view;
    if (!mounted || view == null) return;
    final state = widget.model.host.state;
    final title = switch (widget.model.selection) {
      HomeSelected() => 'Clax',
      AppSelected(:final id) =>
        state.apps.where((a) => a.id == id).firstOrNull?.title ?? 'Clax',
      MakeSelected(:final id) =>
        state.makes.where((m) => m.id == id).firstOrNull?.request ?? 'Clax',
    };
    unawaited(MacWindow.setTitle(title, view: view));
  }

  /// Tells the window what the sidebar takes of it: the material's width
  /// behind a beside-the-detail sidebar, and the toolbar's toggle.
  void _report(double materialWidth) {
    final shown = sidebarShown;
    final changed = _sentShown != shown;
    _sentShown = shown;
    if (changed) widget.onSidebarShown?.call(shown);
    if (!hasDesktopWindowChrome) return;
    widget.model.host.holdToolbar(this, true);
    final view = View.of(context).viewId;
    final moved = _view != view;
    _view = view;
    if (_sentWidth != materialWidth || moved) {
      _sentWidth = materialWidth;
      unawaited(MacWindow.setSidebarWidth(materialWidth, view: view));
    }
    if (changed || moved || _sentEdge != materialWidth) {
      _sentEdge = materialWidth;
      MacToolbar.setLeading(
        [
          ToolbarItemSpec(
            'sidebar',
            symbol: 'sidebar.left',
            label: shown ? 'Hide Sidebar' : 'Show Sidebar',
          ),
        ],
        view: view,
        // Back, Forward and the title start past a beside-the-detail
        // sidebar's edge.
        edge: materialWidth,
      );
    }
  }

  void _showFilter() {
    setState(() {
      if (_wide) {
        _hidden = false;
      } else {
        _over = true;
      }
    });
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _filterFocus.requestFocus();
    });
  }

  @override
  Widget build(BuildContext context) {
    final family = ClaxFamily.of(context);
    final apple = family == ClaxFamily.apple;
    final wide = _wide;
    if (_wasWide != wide) {
      _wasWide = wide;
      _hidden = false;
      _over = false;
    }
    final beside = wide && !_hidden;
    final over = !wide && _over;
    final sideWidth = apple ? _width : ClaxLayout.listPane;
    final material = hasDesktopWindowChrome && apple && beside ? _width : 0.0;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _report(material);
    });

    final detail = _detail(context, left: beside ? sideWidth : 0);
    Widget sidebar() => apple
        ? ClaxSidebar(
            model: widget.model,
            filterFocus: _filterFocus,
            onMenu: widget.onMenu,
            now: widget.now,
            onSelected: over ? () => setState(() => _over = false) : null,
            background: hasDesktopWindowChrome && beside
                ? Colors.transparent
                : _sidebarFill(context),
          )
        : ClaxListPane(model: widget.model);

    Widget body;
    if (beside) {
      body = Stack(
        fit: StackFit.expand,
        children: [
          Row(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              SizedBox(
                key: const ValueKey('split-sidebar'),
                width: sideWidth,
                child: sidebar(),
              ),
              SizedBox(
                width: 1,
                child: ColoredBox(color: claxPalette(context).border),
              ),
              Expanded(child: detail),
            ],
          ),
          if (apple)
            Positioned(
              left: sideWidth - 3,
              width: 7,
              top: 0,
              bottom: 0,
              child: MouseRegion(
                cursor: SystemMouseCursors.resizeColumn,
                child: GestureDetector(
                  key: const ValueKey('split-divider'),
                  behavior: HitTestBehavior.opaque,
                  onHorizontalDragUpdate: (d) =>
                      setState(() => _width = _clamp(_width + d.delta.dx)),
                  onHorizontalDragEnd: (_) =>
                      widget.onSidebarWidth?.call(_width),
                ),
              ),
            ),
        ],
      );
    } else {
      body = Stack(
        fit: StackFit.expand,
        children: [
          detail,
          if (over) ...[
            GestureDetector(
              key: const ValueKey('split-scrim'),
              behavior: HitTestBehavior.opaque,
              onTap: () => setState(() => _over = false),
            ),
            Positioned(
              left: 0,
              top: 0,
              bottom: 0,
              width: sideWidth,
              child: Material(
                key: const ValueKey('split-sidebar'),
                elevation: 8,
                color: _sidebarFill(context),
                child: sidebar(),
              ),
            ),
          ],
        ],
      );
    }

    return CallbackShortcuts(
      bindings: {
        SingleActivator(
          LogicalKeyboardKey.keyS,
          meta: apple,
          control: true,
        ): toggleSidebar,
        SingleActivator(
          LogicalKeyboardKey.keyF,
          meta: apple,
          control: !apple,
        ): _showFilter,
      },
      child: body,
    );
  }

  /// The sidebar's colour where no window material is behind it.
  Color _sidebarFill(BuildContext context) {
    final p = claxPalette(context);
    return Color.alphaBlend(p.fg.withValues(alpha: 0.04), p.bg);
  }

  /// The detail, [left] points from the window's left edge.
  Widget _detail(BuildContext context, {required double left}) {
    final selection = widget.model.selection;
    final home = selection is HomeSelected;
    Widget out = Stack(
      fit: StackFit.expand,
      children: [
        Offstage(
          offstage: !home,
          child: TickerMode(enabled: home, child: widget.home),
        ),
        if (!home)
          KeyedSubtree(
            key: ValueKey(selection),
            child: widget.detailFor(selection),
          ),
      ],
    );
    out = ColoredBox(color: claxPalette(context).bg, child: out);
    final bar = WindowTitlebar.of(context);
    if (bar != null) {
      out = WindowTitlebarScope(
        titlebar: WindowTitlebar(
          top: bar.top,
          leading: math.max(0, bar.leading - left),
        ),
        child: out,
      );
    }
    return out;
  }
}
