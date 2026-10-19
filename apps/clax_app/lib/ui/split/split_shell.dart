import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../home/home_host.dart';
import '../kit/kit.dart';
import '../layout.dart';
import '../mac/mac_toolbar.dart';
import '../mac/window_channel.dart';
import '../shortcuts.dart';
import '../theme.dart';
import '../tokens.g.dart';
import '../user_message.dart';
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
/// The Material family (§3.2) puts a [NavigationRail] of [ClaxLayout.rail]
/// at the leading edge: New app at its top (a mic and "New" on
/// `accent-tint`, [HomeActions.newApp]), then Home, Inbox with the unread
/// count, and Settings. Home is the list pane and the detail at
/// [ClaxWidth.expanded], the detail alone at [ClaxWidth.medium] (the grid
/// with Home selected, an app in its place otherwise); Inbox and Settings
/// show [inbox] and [settings] in place of both ([place]; the home grid
/// stays built under them), and the platform's Back goes Home from them,
/// unless the destination takes it first ([SplitShellState.takeBack]).
/// Without [inbox] or [settings] the rail asks [HomeActions.openInbox] or
/// [HomeActions.openSettings]. Selecting an app or a make goes Home.
///
/// [toggleSidebar] hides and shows it, as ⌃⌘S (Control-S on Material)
/// does. ⌘F (Control-F) shows it and puts the keyboard in its filter;
/// with Home selected in a Mac window whose toolbar has the home app's
/// search item (`search`, [MacToolbar.focusSearch]), ⌘F goes there
/// instead, since that search is the grid's.
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
/// toolbar and the menu bar. The Material family holds it at
/// [ClaxWidth.expanded], where the list pane's search is the grid's
/// ([HomeHost.setQuery]) and the rail carries New app, Inbox and Settings;
/// at [ClaxWidth.medium], with no search beside the grid, the home app
/// keeps its own bar.
///
/// Keys beyond the menus ([claxShortcuts]'s, Command on the Apple family
/// and Control on Material), none of them while a text field has the
/// keyboard:
/// - Space in the sidebar opens what is selected: the keyboard goes into
///   the detail (its first control), and a make shows its build view
///   ([HomeActions.openMake]); a sidebar over the detail goes.
/// - ⌘⌫ asks whether to delete the selected app ([ClaxDialog], Cancel the
///   default) and, on Delete, runs [onDelete]; a failure is a toast.
/// - ⌘1…⌘9 select the first nine starred apps ([SidebarModel.starred]).
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
    this.onDelete,
    this.now = DateTime.now,
    this.inbox,
    this.settings,
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

  /// Deletes app [appId], once the person said to; null leaves ⌘⌫ alone.
  final Future<void> Function(String appId)? onDelete;

  /// The clock type-select reads.
  final DateTime Function() now;

  /// What the Material family's rail shows for Inbox and for Settings
  /// ([SplitPlace]); null asks the home screen's actions instead.
  final WidgetBuilder? inbox, settings;

  /// Whether the window at [context] lays out as a split: from
  /// [ClaxWidth.medium] in either family.
  static bool fits(BuildContext context) =>
      ClaxWidth.of(context) >= ClaxWidth.medium;

  /// The shell [context] is in, if any.
  static SplitShellState? maybeOf(BuildContext context) =>
      context.findAncestorStateOfType<SplitShellState>();

  @override
  State<SplitShell> createState() => SplitShellState();
}

/// What the Material family's rail shows beside it: Home's panes, the
/// inbox or Settings.
enum SplitPlace { home, inbox, settings }

class SplitShellState extends State<SplitShell> {
  final _filterFocus = FocusNode(debugLabel: 'sidebar filter');
  final _detailScope = FocusScopeNode(debugLabel: 'split detail');
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

  var _place = SplitPlace.home;

  /// The rail's destination; always [SplitPlace.home] without a rail.
  SplitPlace get place => _place;

  bool get _rail => ClaxFamily.of(context) == ClaxFamily.material;

  /// What the rail shows for [at]; null for Home's panes.
  WidgetBuilder? _builderOf(SplitPlace at) => switch (at) {
    SplitPlace.home => null,
    SplitPlace.inbox => widget.inbox,
    SplitPlace.settings => widget.settings,
  };

  /// Shows [to] beside the rail, answering whether it could: not without a
  /// rail (the Apple family), nor for a destination with no builder.
  bool go(SplitPlace to) {
    if (!mounted || !_rail) return false;
    if (to != SplitPlace.home && _builderOf(to) == null) return false;
    if (_place != to) setState(() => _place = to);
    return true;
  }

  final _backs = <Object, bool Function()>{};

  /// Lets [owner] (a rail destination showing something in the place of
  /// its own list) take the platform's Back before the rail goes Home:
  /// [back] answers whether it took it. Null lets go.
  void takeBack(Object owner, bool Function()? back) {
    if (back == null) {
      _backs.remove(owner);
    } else {
      _backs[owner] = back;
    }
  }

  /// The platform's Back over a rail destination: the latest taker's, else
  /// Home.
  void _railBack() {
    for (final back in _backs.values.toList().reversed) {
      if (back()) return;
    }
    go(SplitPlace.home);
  }

  /// A rail destination chosen: Home selects the grid as well.
  void _chose(SplitPlace to) {
    final actions = widget.model.host.actions;
    switch (to) {
      case SplitPlace.home:
        go(to);
        widget.model.select(const HomeSelected());
      case SplitPlace.inbox:
        if (!go(to)) actions.openInbox();
      case SplitPlace.settings:
        if (!go(to)) actions.openSettings();
    }
  }

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
    _detailScope.dispose();
    super.dispose();
  }

  /// Whether a text field has the keyboard, where these keys are its own.
  static bool get _typing {
    final context = FocusManager.instance.primaryFocus?.context;
    return context != null &&
        (context.widget is EditableText ||
            context.findAncestorWidgetOfExactType<EditableText>() != null);
  }

  static const _starred = [
    ClaxCommand.starred1,
    ClaxCommand.starred2,
    ClaxCommand.starred3,
    ClaxCommand.starred4,
    ClaxCommand.starred5,
    ClaxCommand.starred6,
    ClaxCommand.starred7,
    ClaxCommand.starred8,
    ClaxCommand.starred9,
  ];

  /// Whether the shell runs [c] now.
  bool _takes(ClaxCommand c) {
    if (_typing) return false;
    if (c == ClaxCommand.openSelected) return true;
    if (c == ClaxCommand.deleteApp) {
      return widget.onDelete != null &&
          widget.model.selection is AppSelected;
    }
    final n = _starred.indexOf(c);
    return n >= 0 && n < widget.model.starred.length;
  }

  void _run(ClaxCommand c) {
    if (c == ClaxCommand.openSelected) return _openSelected();
    if (c == ClaxCommand.deleteApp) {
      if (widget.model.selection case AppSelected(:final id)) {
        unawaited(_delete(id));
      }
      return;
    }
    final n = _starred.indexOf(c);
    final starred = widget.model.starred;
    if (n >= 0 && n < starred.length) {
      widget.model.select(AppSelected(starred[n]));
    }
  }

  /// Space: the keyboard into the detail, and a make's build view.
  void _openSelected() {
    if (_over) setState(() => _over = false);
    if (widget.model.selection case MakeSelected(:final id)) {
      widget.model.host.actions.openMake(id);
    }
    WidgetsBinding.instance.addPostFrameCallback((_) {
      final at = _detailScope.context;
      if (!mounted || at == null) return;
      final policy =
          FocusTraversalGroup.maybeOf(at) ?? ReadingOrderTraversalPolicy();
      (policy.findFirstFocus(_detailScope) ?? _detailScope).requestFocus();
    });
  }

  /// ⌘⌫: asks whether to delete app [id], then deletes it.
  Future<void> _delete(String id) async {
    final title =
        widget.model.host.state.apps
            .where((a) => a.id == id)
            .firstOrNull
            ?.title ??
        'this app';
    final choice = await ClaxDialog.show(
      context,
      title: 'Delete “$title”?',
      body: 'Its versions, threads and saved data go with it. You can’t undo '
          'this.',
      actions: const [
        ClaxDialogAction('Delete', kind: ClaxButtonKind.destructive),
        ClaxDialogAction('Cancel'),
      ],
      defaultIndex: 1,
      cancelIndex: 1,
    );
    if (choice != 0 || !mounted) return;
    try {
      await widget.onDelete?.call(id);
    } on Object catch (e) {
      if (mounted) ClaxToast.show(context, userMessage(e), error: true);
    }
  }

  void _onSelected() {
    if (!mounted) return;
    setState(() => _place = SplitPlace.home);
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
  /// behind a beside-the-detail sidebar, and the toolbar's toggle; and
  /// the home app whether the window holds its toolbar.
  void _report(double materialWidth) {
    final shown = sidebarShown;
    final changed = _sentShown != shown;
    _sentShown = shown;
    if (changed) widget.onSidebarShown?.call(shown);
    final host = widget.model.host;
    final rail = _rail;
    final holds = hasDesktopWindowChrome || (rail && _wide);
    host.holdToolbar(this, holds);
    // The list pane's search is the grid's.
    if (holds && rail) host.setQuery(widget.model.filter);
    if (!hasDesktopWindowChrome) return;
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

  /// ⌘F: the toolbar's search with Home selected, when the window's
  /// toolbar has one; the sidebar's filter otherwise.
  Future<void> _find() async {
    final view = _view;
    if (hasDesktopWindowChrome &&
        view != null &&
        widget.model.selection is HomeSelected &&
        await MacToolbar.focusSearch('search', view: view)) {
      return;
    }
    if (mounted) _showFilter();
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
    final keys = claxShortcuts(family);
    Map<ShortcutActivator, Intent> only(bool Function(ClaxCommand) which) => {
      for (final MapEntry(:key, :value) in keys.entries)
        if (value is ClaxCommandIntent && which(value.command)) key: value,
    };
    Widget sidebar() => Shortcuts(
      shortcuts: only((c) => c == ClaxCommand.openSelected),
      child: apple
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
        : ClaxListPane(
            model: widget.model,
            searchFocus: _filterFocus,
            onSelected: over ? () => setState(() => _over = false) : null,
          ),
    );

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

    if (!apple) body = _railed(context, body);
    body = Shortcuts(
      shortcuts: only(
        (c) => c == ClaxCommand.deleteApp || _starred.contains(c),
      ),
      child: Actions(
        actions: {ClaxCommandIntent: _ShellCommand(this)},
        child: body,
      ),
    );
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
        ): () => unawaited(_find()),
      },
      // The Material family's list pane takes no focus of its own (the
      // Apple family's sidebar does), so the shell starts with the
      // keyboard for these keys to be heard.
      child: apple ? body : Focus(autofocus: true, child: body),
    );
  }

  /// [home] (the list pane and the detail) or the rail's destination in
  /// its place, beside the rail.
  Widget _railed(BuildContext context, Widget home) {
    final builder = _builderOf(_place);
    final shown = builder == null ? SplitPlace.home : _place;
    return PopScope(
      canPop: shown == SplitPlace.home,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop) _railBack();
      },
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _ClaxRail(
            host: widget.model.host,
            place: shown,
            onChosen: _chose,
          ),
          Expanded(
            child: Stack(
              fit: StackFit.expand,
              children: [
                Offstage(
                  offstage: shown != SplitPlace.home,
                  child: TickerMode(
                    enabled: shown == SplitPlace.home,
                    child: home,
                  ),
                ),
                if (builder != null)
                  KeyedSubtree(
                    key: ValueKey(shown),
                    child: ColoredBox(
                      color: claxPalette(context).bg,
                      child: Builder(builder: builder),
                    ),
                  ),
              ],
            ),
          ),
        ],
      ),
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
    out = ColoredBox(
      key: const ValueKey('split-detail'),
      color: claxPalette(context).bg,
      child: out,
    );
    out = FocusScope(node: _detailScope, child: out);
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

/// The commands a [SplitShell] runs ([SplitShellState._takes]); any other
/// goes on to the actions above it.
class _ShellCommand extends Action<ClaxCommandIntent> {
  _ShellCommand(this.shell);

  final SplitShellState shell;

  @override
  bool isEnabled(ClaxCommandIntent intent) =>
      shell.mounted && shell._takes(intent.command);

  @override
  Object? invoke(ClaxCommandIntent intent) {
    shell._run(intent.command);
    return null;
  }
}

/// The Material family's navigation rail, [ClaxLayout.rail] wide: New app
/// at the top, then Home, Inbox with [host]'s unread count, and Settings,
/// [place] selected.
class _ClaxRail extends StatelessWidget {
  const _ClaxRail({
    required this.host,
    required this.place,
    required this.onChosen,
  });

  final HomeHost host;
  final SplitPlace place;
  final ValueChanged<SplitPlace> onChosen;

  @override
  Widget build(BuildContext context) {
    final p = claxPalette(context);
    return SizedBox(
      key: const ValueKey('split-rail'),
      width: ClaxLayout.rail,
      child: ListenableBuilder(
        listenable: host,
        builder: (context, _) => NavigationRail(
          backgroundColor: p.bg,
          minWidth: ClaxLayout.rail,
          labelType: NavigationRailLabelType.all,
          groupAlignment: -1,
          selectedIndex: place.index,
          onDestinationSelected: (i) => onChosen(SplitPlace.values[i]),
          leading: Padding(
            padding: const EdgeInsets.only(
              top: ClaxSpace.s4,
              bottom: ClaxSpace.s16,
            ),
            child: _NewApp(onPressed: host.actions.newApp),
          ),
          destinations: [
            const NavigationRailDestination(
              icon: Icon(Icons.home_outlined),
              selectedIcon: Icon(Icons.home),
              label: Text('Home'),
            ),
            NavigationRailDestination(
              icon: ClaxBadge(
                count: host.unread,
                child: const Icon(Icons.inbox_outlined),
              ),
              selectedIcon: ClaxBadge(
                count: host.unread,
                child: const Icon(Icons.inbox),
              ),
              label: const Text('Inbox'),
            ),
            const NavigationRailDestination(
              icon: Icon(Icons.settings_outlined),
              selectedIcon: Icon(Icons.settings),
              label: Text('Settings'),
            ),
          ],
        ),
      ),
    );
  }
}

/// The rail's New app: a mic over "New", on `accent-tint`.
class _NewApp extends StatelessWidget {
  const _NewApp({required this.onPressed});

  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) {
    final p = claxPalette(context);
    return Semantics(
      button: true,
      label: 'New app',
      excludeSemantics: true,
      onTap: onPressed,
      child: Tooltip(
        message: 'New app',
        child: Material(
          key: const ValueKey('rail-new'),
          color: p.accentTint,
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(ClaxSpace.s16),
          ),
          clipBehavior: Clip.antiAlias,
          child: InkWell(
            onTap: onPressed,
            child: SizedBox(
              width: 56,
              height: 56,
              child: Column(
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  Icon(Icons.mic_none, color: p.agentInk),
                  Text(
                    'New',
                    style: Theme.of(
                      context,
                    ).textTheme.labelMedium?.copyWith(color: p.agentInk),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}
