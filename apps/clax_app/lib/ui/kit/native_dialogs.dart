import 'dart:async';

import 'package:flutter/services.dart';

import 'menu.dart';

/// The Mac's own menus and alerts (`macos/Runner/NativeDialogs.swift`),
/// over the method channel `dev.empathic.clax/dialogs`. Callers check
/// `hasDesktopWindowChrome` first; where nothing answers on the channel (a
/// test, another platform) each call throws [MissingPluginException] and
/// the caller draws its own.
abstract final class NativeDialogs {
  static const channel = MethodChannel('dev.empathic.clax/dialogs');

  static var _next = 0;

  /// Pops up an `NSMenu` of [items] with its top left at [at], in the
  /// window's Flutter coordinates (logical pixels from the top left), and
  /// answers the chosen item's ID, or null when the menu closed without a
  /// choice.
  static Future<String?> menu(List<ClaxMenuItem> items, Offset at) =>
      channel.invokeMethod<String>('menu', {
        'items': [
          for (final i in items)
            if (i.separator)
              {'separator': true}
            else
              {
                'id': i.id,
                'title': i.title,
                'key': i.key,
                'symbol': i.symbol,
                'detail': i.detail,
                'checked': i.checked,
                'destructive': i.destructive,
                'enabled': i.enabled,
              },
        ],
        'at': {'x': at.dx, 'y': at.dy},
      });

  /// Runs an `NSAlert` as a sheet on the key window and answers the
  /// [index] into [buttons] of the one pressed, and whether its
  /// [suppression] checkbox was checked.
  ///
  /// - [style] is `informational`, `warning` or `critical`.
  /// - [buttons] are in reading order; the Mac lays them out with the
  ///   first on the left. [defaultIndex] answers Return and [cancelIndex]
  ///   Escape.
  /// - [suppressedButtons], beside [buttons], are their labels while the
  ///   checkbox is checked (null keeps the label).
  /// - [destructive] lists the buttons drawn as destructive.
  /// - With [armMs], the default button is disabled for that long from the
  ///   sheet's first display, and Return held from before it armed answers
  ///   nothing.
  /// - When [dismissed] completes first, the sheet is taken down and the
  ///   answer is [cancelIndex] (null without one).
  static Future<({int? index, bool suppressed})> alert({
    required String style,
    required String title,
    String? body,
    required List<String> buttons,
    List<String?>? suppressedButtons,
    List<int> destructive = const [],
    int? defaultIndex,
    int? cancelIndex,
    int? armMs,
    String? suppression,
    Future<void>? dismissed,
  }) async {
    assert(const {'informational', 'warning', 'critical'}.contains(style));
    assert(
      suppressedButtons == null || suppressedButtons.length == buttons.length,
    );
    final id = _next++;
    var answered = false;
    unawaited(
      dismissed?.then((_) {
        if (answered) return;
        unawaited(
          channel
              .invokeMethod<void>('dismissAlert', {'id': id})
              .catchError((Object _) {}),
        );
      }),
    );
    try {
      final got = await channel.invokeMapMethod<String, Object?>('alert', {
        'id': id,
        'style': style,
        'title': title,
        'body': body,
        'buttons': buttons,
        'suppressedButtons': suppressedButtons,
        'destructive': destructive,
        'defaultIndex': defaultIndex,
        'cancelIndex': cancelIndex,
        'armMs': armMs,
        'suppression': suppression,
      });
      return (
        index: got?['index'] as int?,
        suppressed: got?['suppressed'] as bool? ?? false,
      );
    } finally {
      answered = true;
    }
  }
}
