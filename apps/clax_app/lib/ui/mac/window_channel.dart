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
/// - `accentColor` {light, dark}: the system's accent changed, each
///   `#rrggbb` in that appearance; [accent]. It names no view: the accent
///   is every window's.
///
/// To it, besides the setters: `dragWindow` {view} ([dragWindow]), and
/// `accentColor`, which answers the accent as the native side sends it.
abstract final class MacWindow {
  static const channel = MethodChannel('dev.empathic.clax/window');

  static final _escapes = StreamController<int>.broadcast();
  static final _titlebars = <int, ValueNotifier<WindowTitlebar>>{};
  static final _wanting = <int, Set<Object>>{};
  static final _wantedSent = <int, bool>{};
  static final _accent = ValueNotifier<SystemAccent?>(null);
  static var _accentAsked = false;
  static var _listening = false;

  /// The system's accent (`NSColor.controlAccentColor`) in the light and
  /// the dark appearance; null until the native side says, and wherever
  /// it does not ([hasDesktopWindowChrome] false, or no bridge). The first
  /// use asks it; later changes arrive from the native side.
  static ValueListenable<SystemAccent?> get accent {
    _listen();
    if (!_accentAsked && hasDesktopWindowChrome) {
      _accentAsked = true;
      unawaited(
        channel
            .invokeMethod<Object?>('accentColor')
            .then((answer) {
              if (_readAccent(answer) case final a?) _accent.value = a;
            })
            .catchError((Object _) {}),
      );
    }
    return _accent;
  }

  /// Forgets the accent, so the next [accent] asks again.
  @visibleForTesting
  static void resetAccent() {
    _accentAsked = false;
    _accent.value = null;
  }

  /// An accent as the native side sends it: {light, dark}, each `#rrggbb`.
  static SystemAccent? _readAccent(Object? map) => switch (map) {
    {'light': final String light, 'dark': final String dark} => switch ((
      _hex(light),
      _hex(dark),
    )) {
      (final l?, final d?) => (light: l, dark: d),
      _ => null,
    },
    _ => null,
  };

  static Color? _hex(String s) {
    if (s.length != 7 || !s.startsWith('#')) return null;
    final v = int.tryParse(s.substring(1), radix: 16);
    return v == null ? null : Color(0xFF000000 | v);
  }

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
      if (call.method == 'accentColor') {
        final a = _readAccent(args);
        if (a == null) return false;
        _accent.value = a;
        return true;
      }
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

  /// The mouse went down on no control in [view]'s window's toolbar row:
  /// the window follows the mouse while the button is held (its title
  /// bar's double click action, on the second press of a double click).
  static Future<void> dragWindow({required int view}) =>
      _send('dragWindow', {'view': view});

  /// Makes [view]'s window's content [height] points tall, its top edge
  /// where it was, animated unless [animate] is false: a Settings window
  /// fits the tab it shows. No taller than the screen allows.
  static Future<void> setContentHeight(
    double height, {
    required int view,
    bool animate = true,
  }) => _send('setContentHeight', {
    'view': view,
    'height': height,
    'animate': animate,
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
