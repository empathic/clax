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
/// - `kind` {view} answers the kind of window showing that view (`main`,
///   `settings` or `inbox`), or null for a view no window shows: the
///   engine's implicit view, which stays empty while the engine runs
///   several views.
/// - `open` {kind} brings the window of that kind forward, making it when
///   there is none ([open]).
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

  /// Brings the window of [kind] (`main`, `settings` or `inbox`) forward,
  /// making it when there is none; whether one shows. False without the
  /// native side (a test, or another platform).
  static Future<bool> open(String kind) async {
    try {
      return await channel.invokeMethod<bool>('open', {'kind': kind}) ?? false;
    } on MissingPluginException {
      return false;
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

/// What the Mac app's further windows (Settings, the Inbox) show, by kind:
/// set by the app's root once its state is up, so every window shares its
/// services, and read by [ClaxViews]. Each builder is given the window's
/// view ID.
class WindowContents extends ChangeNotifier {
  final _builders = <String, Widget Function(int view)>{};

  /// Makes [builder] what windows of [kind] show; null shows nothing.
  void set(String kind, Widget Function(int view)? builder) {
    if (builder == null) {
      if (_builders.remove(kind) == null) return;
    } else {
      _builders[kind] = builder;
    }
    notifyListeners();
  }

  /// What a window of [kind] showing [view] shows; null for nothing yet.
  Widget? build(String kind, int view) => _builders[kind]?.call(view);
}

/// The root of the Mac app, given to `runWidget`: one [View] for each
/// window, by the kind [MacWindows.kind] names. The main window shows
/// [main]; a further window (Settings, the Inbox) shows what [further]
/// gives its kind. A view no window shows is not drawn. Content runs under the
/// window's transparent toolbar row and is padded below it
/// ([MacWindow.titlebar]), as a phone's is below its status bar; a screen
/// that draws its bar in that row, beside the window buttons, finds it
/// with [WindowTitlebar.of]. A press in the row that no control takes
/// moves the window ([WindowRowDrag]).
class ClaxViews extends StatefulWidget {
  const ClaxViews({super.key, required this.main, this.further});

  /// The main window's content.
  final Widget main;

  /// What the further windows show; none when null.
  final WindowContents? further;

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
  Widget build(BuildContext context) {
    final further = widget.further;
    if (further == null) return _views();
    return ListenableBuilder(
      listenable: further,
      builder: (context, _) => _views(),
    );
  }

  Widget _views() => ViewCollection(
    views: [
      for (final v in WidgetsBinding.instance.platformDispatcher.views)
        if (_content(v) case final content?)
          View(
            key: ValueKey(v.viewId),
            view: v,
            child: _BelowTitlebar(view: v, child: content),
          ),
    ],
  );

  /// What [v]'s window shows; null when no window shows it, or nothing is
  /// given for its kind yet.
  Widget? _content(FlutterView v) => switch (_kinds[v.viewId]) {
    null => null,
    'main' => widget.main,
    final kind => widget.further?.build(kind, v.viewId),
  };
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
