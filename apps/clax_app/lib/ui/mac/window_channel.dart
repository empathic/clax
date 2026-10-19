import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import '../kit/family.dart';
import '../kit/window_titlebar.dart';

/// A Mac window's chrome (`macos/Runner/WindowBridge.swift`) over the
/// method channel `dev.empathic.clax/window`. Every window shows one
/// Flutter view, and every call names the view whose window it concerns
/// (`View.of(context).viewId`). Calls are no-ops unless
/// [hasDesktopWindowChrome], and a native side that does not answer is no
/// error.
///
/// From the native side:
/// - `escape` {view}: Escape was pressed while a page's WebView had the
///   keyboard in that window, which wanted it ([setEscapeWanted]); the
///   event went no further. Arrives on [escapes].
/// - `titlebar` {view, top, leading}: the window's title bar and toolbar
///   row, which its content runs under, and the right edge of its window
///   buttons; [titlebar].
abstract final class MacWindow {
  static const channel = MethodChannel('dev.empathic.clax/window');

  static final _escapes = StreamController<int>.broadcast();
  static final _titlebars = <int, ValueNotifier<WindowTitlebar>>{};
  static final _wanting = <int, Set<Object>>{};
  static final _wantedSent = <int, bool>{};
  static var _listening = false;

  /// The views whose window caught an Escape from a page.
  static Stream<int> get escapes {
    _listen();
    return _escapes.stream;
  }

  /// [view]'s window's title bar; [WindowTitlebar.none] until the native
  /// side says. The first use asks it.
  static ValueListenable<WindowTitlebar> titlebar(int view) {
    _listen();
    return _titlebars.putIfAbsent(view, () {
      final bar = ValueNotifier(WindowTitlebar.none);
      if (hasDesktopWindowChrome) {
        unawaited(
          channel
              .invokeMethod<Object?>('titlebar', {'view': view})
              .then((answer) {
                if (_read(answer) case final b?) bar.value = b;
              })
              .catchError((Object _) {}),
        );
      }
      return bar;
    });
  }

  /// A title bar as the native side sends it: {top, leading}.
  static WindowTitlebar? _read(Object? map) => switch (map) {
    {'top': final num top, 'leading': final num leading} => WindowTitlebar(
      top: top.toDouble(),
      leading: leading.toDouble(),
    ),
    _ => null,
  };

  static void _listen() {
    if (_listening) return;
    _listening = true;
    channel.setMethodCallHandler((call) async {
      final args = call.arguments;
      if (args is! Map || args['view'] is! int) return false;
      final view = args['view'] as int;
      switch (call.method) {
        case 'escape':
          _escapes.add(view);
          return _wanting[view]?.isNotEmpty ?? false;
        case 'titlebar':
          final bar = _read(args);
          if (bar == null) return false;
          _titlebars
                  .putIfAbsent(view, () => ValueNotifier(WindowTitlebar.none))
                  .value =
              bar;
          return true;
      }
      return false;
    });
  }

  /// Shows [title] in [view]'s window, with [subtitle] under it (none when
  /// null).
  static Future<void> setTitle(
    String title, {
    String? subtitle,
    required int view,
  }) => _send('setTitle', {
    'view': view,
    'title': title,
    'subtitle': subtitle ?? '',
  });

  /// Shows the sidebar material behind the leftmost [width] points of
  /// [view]'s window, where the Flutter view draws nothing; 0 hides it.
  static Future<void> setSidebarWidth(double width, {required int view}) =>
      _send('setSidebarWidth', {'view': view, 'width': width});

  /// Whether [owner] wants Escape in [view]'s window while a page's
  /// WebView has the keyboard: a panel or a mode Escape closes is up. The
  /// window takes Escape from the page while any owner in it wants it, and
  /// sends it to [escapes] instead.
  static void setEscapeWanted(Object owner, bool on, {required int view}) {
    if (!hasDesktopWindowChrome) return;
    final owners = _wanting.putIfAbsent(view, () => {});
    on ? owners.add(owner) : owners.remove(owner);
    final wanted = owners.isNotEmpty;
    if (!wanted) _wanting.remove(view);
    if ((_wantedSent[view] ?? false) == wanted) return;
    _wantedSent[view] = wanted;
    unawaited(_send('setEscapeWanted', {'view': view, 'on': wanted}));
  }

  static Future<void> _send(String method, Map<String, Object> args) async {
    if (!hasDesktopWindowChrome) return;
    try {
      await channel.invokeMethod<void>(method, args);
    } on MissingPluginException {
      // No window chrome here (a test, or a build without the bridge).
    }
  }
}
