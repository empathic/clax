import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';

import '../kit/family.dart';
import '../mac/mac_toolbar.dart';
import '../mac/window_channel.dart';
import '../settings_screen.dart';
import '../theme.dart';

/// The tabs of the Mac's Settings window, in order, each with its title
/// (the window's title while it shows) and its SF Symbol in the window's
/// toolbar. The phone's Settings shows the same sections in one column
/// (`SettingsScreen` without a tab).
enum SettingsTab {
  /// Clax's and the home screen's versions, the licences and a haiku.
  general('General', 'gearshape'),

  /// The API key, the agent and the model.
  claude('Claude', 'key'),

  /// The two dictation choices.
  dictation('Dictation', 'mic'),

  /// Your name and your sharing key.
  sharing('Sharing', 'person.2'),

  /// The system setting's state, and Open System Settings.
  notifications('Notifications', 'bell'),

  /// Open in comment mode, Undo, Reset and the built-in home screen.
  homeScreen('Home Screen', 'house'),

  /// The Developer switch, and Developer tools when it is on.
  advanced('Advanced', 'wrench.and.screwdriver');

  const SettingsTab(this.title, this.symbol);

  /// The tab's name: its toolbar label and the window's title.
  final String title;

  /// The SF Symbol of its toolbar item.
  final String symbol;
}

/// The Mac's Settings window: one [SettingsTab] at a time, the first
/// [tab], shown by [screen] as a grouped form. The window's toolbar holds
/// the tabs (selectable items, the shown one selected), the window's title
/// is the shown tab's, and the window's height follows each tab's content
/// ([MacWindow.setContentHeight]), scrolling only where the screen is too
/// short. A click on another tab in this window's toolbar shows it.
/// [onClosed] runs when the window goes.
class SettingsTabs extends StatefulWidget {
  const SettingsTabs({
    super.key,
    required this.tab,
    required this.screen,
    this.onClosed,
  });

  /// The tab shown first.
  final SettingsTab tab;

  /// The page for a tab; the same screen for each, so what it read stays
  /// as the tabs change.
  final SettingsScreen Function(SettingsTab tab) screen;

  final VoidCallback? onClosed;

  @override
  State<SettingsTabs> createState() => _SettingsTabsState();
}

class _SettingsTabsState extends State<SettingsTabs> {
  late SettingsTab _tab = widget.tab;
  StreamSubscription<ToolbarClick>? _clicks;
  int? _view;
  double? _height;

  @override
  void initState() {
    super.initState();
    if (hasDesktopWindowChrome) _clicks = MacToolbar.clicks.listen(_clicked);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final view = View.of(context).viewId;
    if (view == _view) return;
    _view = view;
    _height = null;
    _declare();
  }

  @override
  void dispose() {
    unawaited(_clicks?.cancel());
    if (_view case final view?) MacToolbar.clear(this, view: view);
    widget.onClosed?.call();
    super.dispose();
  }

  void _declare() {
    final view = _view;
    if (view == null) return;
    MacToolbar.setItems(this, [
      for (final t in SettingsTab.values)
        ToolbarItemSpec(
          t.name,
          symbol: t.symbol,
          label: t.title,
          select: true,
          on: t == _tab,
        ),
    ], view: view);
    unawaited(MacWindow.setTitle(_tab.title, view: view));
  }

  void _clicked(ToolbarClick c) {
    if (!mounted || c.view != _view) return;
    final tab = SettingsTab.values.where((t) => t.name == c.id).firstOrNull;
    if (tab == null || tab == _tab) return;
    setState(() => _tab = tab);
    _declare();
  }

  void _fit(Size size) {
    final view = _view;
    if (view == null || !mounted) return;
    final h = size.height.ceilToDouble();
    if (h == _height) return;
    final first = _height == null;
    _height = h;
    unawaited(MacWindow.setContentHeight(h, view: view, animate: !first));
  }

  @override
  Widget build(BuildContext context) => Scaffold(
    backgroundColor: claxPalette(context).bg,
    body: SingleChildScrollView(
      child: _SizeReport(onSize: _fit, child: widget.screen(_tab)),
    ),
  );
}

/// [child], telling [onSize] its size after each layout that changed it.
class _SizeReport extends SingleChildRenderObjectWidget {
  const _SizeReport({required this.onSize, required super.child});

  final ValueChanged<Size> onSize;

  @override
  RenderObject createRenderObject(BuildContext context) => _RenderSizeReport(onSize);

  @override
  void updateRenderObject(BuildContext context, _RenderSizeReport r) =>
      r.onSize = onSize;
}

class _RenderSizeReport extends RenderProxyBox {
  _RenderSizeReport(this.onSize);

  ValueChanged<Size> onSize;
  Size? _told;

  @override
  void performLayout() {
    super.performLayout();
    final now = size;
    if (now == _told) return;
    _told = now;
    WidgetsBinding.instance.addPostFrameCallback((_) => onSize(now));
  }
}
