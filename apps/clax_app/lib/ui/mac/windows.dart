import 'dart:async';
import 'dart:ui'
    show FlutterView, ViewFocusDirection, ViewFocusEvent, ViewFocusState;

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../kit/window_titlebar.dart';
import 'window_channel.dart';
import 'window_drag.dart';

/// The Mac app's windows (`macos/Runner/WindowBridge.swift`) over the
/// method channel `dev.empathic.clax/windows`. Each window shows one view
/// of the app's one Flutter engine:
/// - `kind` {view} answers the kind of window showing that view (`main`),
///   or null for a view no window shows: the engine's implicit view, which
///   stays empty while the engine runs several views.
/// - `focused` {view}, from the native side: that view's window became the
///   key window, so Flutter's one primary focus moves into that view.
abstract final class MacWindows {
  static const channel = MethodChannel('dev.empathic.clax/windows');

  /// The kind of window showing [view], or null for none. Without the
  /// native side (a test, a build without it) the implicit view is the
  /// main window, as it is when the engine runs a single view.
  static Future<String?> kind(int view) async {
    try {
      return await channel.invokeMethod<String>('kind', view);
    } on MissingPluginException {
      final implicit = WidgetsBinding.instance.platformDispatcher.implicitView;
      return implicit?.viewId == view ? 'main' : null;
    }
  }

  /// Takes the native side's `focused` calls.
  static void install() => channel.setMethodCallHandler((call) async {
    if (call.method != 'focused') return false;
    final view = switch (call.arguments) {
      {'view': final int v} => v,
      _ => null,
    };
    if (view == null) return false;
    WidgetsBinding.instance.handleViewFocusChanged(
      ViewFocusEvent(
        viewId: view,
        state: ViewFocusState.focused,
        direction: ViewFocusDirection.undefined,
      ),
    );
    return true;
  });
}

/// The root of the Mac app, given to `runWidget`: one [View] for each
/// window, by the kind [MacWindows.kind] names. The main window shows
/// [main]. A view no window shows is not drawn. Content runs under the
/// window's transparent toolbar row and is padded below it
/// ([MacWindow.titlebar]), as a phone's is below its status bar; a screen
/// that draws its bar in that row, beside the window buttons, finds it
/// with [WindowTitlebar.of]. A press in the row that no control takes
/// moves the window ([WindowRowDrag]).
class ClaxViews extends StatefulWidget {
  const ClaxViews({super.key, required this.main});

  /// The main window's content.
  final Widget main;

  @override
  State<ClaxViews> createState() => _ClaxViewsState();
}

class _ClaxViewsState extends State<ClaxViews> with WidgetsBindingObserver {
  /// Each view's window kind; null for none, absent until answered.
  final _kinds = <int, String?>{};

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    _sync();
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  /// Views come and go with windows; each one's metrics change brings it.
  @override
  void didChangeMetrics() => _sync();

  void _sync() {
    final views = WidgetsBinding.instance.platformDispatcher.views;
    for (final v in views) {
      if (_kinds.containsKey(v.viewId)) continue;
      _kinds[v.viewId] = null;
      unawaited(
        MacWindows.kind(v.viewId).then((k) {
          if (mounted && k != null) setState(() => _kinds[v.viewId] = k);
        }),
      );
    }
    final ids = {for (final v in views) v.viewId};
    final gone = _kinds.keys.where((id) => !ids.contains(id)).toList();
    if (gone.isNotEmpty) setState(() => gone.forEach(_kinds.remove));
  }

  @override
  Widget build(BuildContext context) => ViewCollection(
    views: [
      for (final v in WidgetsBinding.instance.platformDispatcher.views)
        if (_kinds[v.viewId] == 'main')
          View(
            key: ValueKey(v.viewId),
            view: v,
            child: _BelowTitlebar(view: v, child: widget.main),
          ),
    ],
  );
}

/// [child] with the window's title bar height added to its top padding,
/// the title bar in a [WindowTitlebarScope], and the row moving the
/// window.
class _BelowTitlebar extends StatelessWidget {
  const _BelowTitlebar({required this.view, required this.child});

  final FlutterView view;
  final Widget child;

  @override
  Widget build(BuildContext context) => ValueListenableBuilder<WindowTitlebar>(
    valueListenable: MacWindow.titlebar(view.viewId),
    // The same widgets whatever the title bar, so the app's state
    // stays when it arrives or changes.
    builder: (context, bar, child) {
      final top = bar.top;
      final data = MediaQuery.of(context);
      EdgeInsets below(EdgeInsets e) =>
          e.copyWith(top: e.top > top ? e.top : top);
      return WindowTitlebarScope(
        titlebar: bar,
        child: MediaQuery(
          data: data.copyWith(
            padding: below(data.padding),
            viewPadding: below(data.viewPadding),
          ),
          child: WindowRowDrag(
            titlebar: bar,
            onDrag: () => MacWindow.dragWindow(view: view.viewId),
            child: child!,
          ),
        ),
      );
    },
    child: child,
  );
}
