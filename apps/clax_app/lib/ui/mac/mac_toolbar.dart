import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import '../bundle_sheets.dart';
import '../kit/family.dart';

/// One item of a Mac window's toolbar, as Flutter declares it: [symbol]
/// is an SF Symbol's name and [label] its name in "Icon and Text" mode,
/// its tooltip and what VoiceOver reads. [on] draws the symbol filled
/// (its `.fill` form), tinted [tint] when given; [badge] is a count on
/// it, none when null or 0. Consecutive items with the same [group] are
/// one segmented control (Back and Forward); a group named `navigation`
/// sits before the window's title.
@immutable
final class ToolbarItemSpec {
  const ToolbarItemSpec(
    this.id, {
    required this.symbol,
    required this.label,
    this.enabled = true,
    this.on = false,
    this.badge,
    this.tint,
    this.group,
  });

  final String id, symbol, label;
  final bool enabled, on;
  final int? badge;
  final Color? tint;
  final String? group;

  /// As sent over the channel.
  Map<String, Object> toMap() => {
    'id': id,
    'symbol': symbol,
    'label': label,
    'enabled': enabled,
    'on': on,
    if (badge case final b? when b > 0) 'badge': b,
    if (tint case final t?) 'tint': _hex(t),
    'group': ?group,
  };

  static String _hex(Color c) {
    String two(double v) =>
        (v * 255).round().clamp(0, 255).toRadixString(16).padLeft(2, '0');
    return '#${two(c.r)}${two(c.g)}${two(c.b)}';
  }

  @override
  bool operator ==(Object other) =>
      other is ToolbarItemSpec &&
      other.id == id &&
      other.symbol == symbol &&
      other.label == label &&
      other.enabled == enabled &&
      other.on == on &&
      other.badge == badge &&
      other.tint == tint &&
      other.group == group;

  @override
  int get hashCode =>
      Object.hash(id, symbol, label, enabled, on, badge, tint, group);

  @override
  String toString() => 'ToolbarItemSpec($id)';
}

/// An item of [view]'s window's toolbar was clicked.
@immutable
final class ToolbarClick {
  const ToolbarClick(this.view, this.id);

  final int view;
  final String id;

  @override
  bool operator ==(Object other) =>
      other is ToolbarClick && other.view == view && other.id == id;

  @override
  int get hashCode => Object.hash(view, id);

  @override
  String toString() => 'ToolbarClick($view, $id)';
}

/// A Mac window's native toolbar (`macos/Runner/Toolbar.swift`) over the
/// method channel `dev.empathic.clax/toolbar`. Calls are no-ops unless
/// [hasDesktopWindowChrome], and a native side that does not answer is no
/// error.
///
/// A screen that the toolbar carries declares its items with [setItems];
/// the window then shows its title (`MacWindow.setTitle`) after the
/// `navigation` group, and the toolbar row is the toolbar's, which drags
/// the window where it has no item. Screens that draw their own bar in
/// the row (`ClaxScaffold`'s Mac header, the launcher) declare none. Each
/// window has one declaring owner at a time, the last to call
/// [setItems]; [clear] by any other owner changes nothing, so a screen
/// going away cannot take the next screen's items. A window's leading
/// items ([setLeading], such as the sidebar's toggle) go before its
/// owner's, while it has an owner that declares any.
///
/// - `setItems` {view, items: [ToolbarItemSpec.toMap…]}: the window's
///   items, in order; none hides the title.
/// - `share` {view, item, path}: the share picker for the file at [path],
///   from item [item]; answers whether a target was chosen.
/// - From the native side, `clicked` {view, id}: arrives on [clicks].
abstract final class MacToolbar {
  static const channel = MethodChannel('dev.empathic.clax/toolbar');

  static final _clicks = StreamController<ToolbarClick>.broadcast();
  static final _owners = <int, Object>{};
  static final _declared = <int, List<ToolbarItemSpec>>{};
  static final _leading = <int, List<ToolbarItemSpec>>{};
  static final _sent = <int, List<ToolbarItemSpec>>{};
  static var _listening = false;

  /// The items people click, in any window.
  static Stream<ToolbarClick> get clicks {
    _listen();
    return _clicks.stream;
  }

  /// Whether [owner] declares [view]'s window's items now.
  static bool owns(Object owner, {required int view}) =>
      identical(_owners[view], owner);

  /// Makes [owner] the one declaring [view]'s window's items, and [items]
  /// those items. Sent only when they change.
  static void setItems(
    Object owner,
    List<ToolbarItemSpec> items, {
    required int view,
  }) {
    if (!hasDesktopWindowChrome) return;
    _listen();
    _owners[view] = owner;
    _declared[view] = List.of(items);
    _send(view);
  }

  /// Makes [items] the ones [view]'s window shows before its owner's
  /// ([setItems]); none while no owner declares any.
  static void setLeading(List<ToolbarItemSpec> items, {required int view}) {
    if (!hasDesktopWindowChrome) return;
    _listen();
    _leading[view] = List.of(items);
    if (_owners.containsKey(view)) _send(view);
  }

  /// Takes [owner]'s items off [view]'s window, if it still declares them.
  static void clear(Object owner, {required int view}) {
    if (!hasDesktopWindowChrome || !owns(owner, view: view)) return;
    _owners.remove(view);
    _declared.remove(view);
    _send(view);
  }

  static void _send(int view) {
    final own = _declared[view] ?? const <ToolbarItemSpec>[];
    final items = [
      if (own.isNotEmpty) ...?_leading[view],
      ...own,
    ];
    final last = _sent[view];
    if (last != null && listEquals(last, items)) return;
    _sent[view] = List.of(items);
    unawaited(
      _invoke<void>('setItems', {
        'view': view,
        'items': [for (final i in items) i.toMap()],
      }),
    );
  }

  /// The share picker for the file at [path], shown from [item] of [view]'s
  /// window's toolbar; whether the person chose a target.
  static Future<bool> share(
    String path, {
    required String item,
    required int view,
  }) async {
    if (!hasDesktopWindowChrome) return false;
    return await _invoke<bool>('share', {
          'view': view,
          'item': item,
          'path': path,
        }) ??
        false;
  }

  static Future<T?> _invoke<T>(String method, Map<String, Object> args) async {
    try {
      return await channel.invokeMethod<T>(method, args);
    } on MissingPluginException {
      // No native toolbar here (a test, or a build without it).
      return null;
    }
  }

  static void _listen() {
    if (_listening) return;
    _listening = true;
    channel.setMethodCallHandler((call) async {
      if (call.method != 'clicked') return false;
      final click = switch (call.arguments) {
        {'view': final int view, 'id': final String id} => ToolbarClick(
          view,
          id,
        ),
        _ => null,
      };
      if (click == null) return false;
      _clicks.add(click);
      return true;
    });
  }

  /// Forgets every owner and what was sent; tests only.
  @visibleForTesting
  static void reset() {
    _owners.clear();
    _declared.clear();
    _leading.clear();
    _sent.clear();
    _listening = false;
  }
}

/// [BundleFiles] for the toolbar's Share item of [view]'s window: a built
/// bundle is written to a directory of its own under the temporary
/// directory and handed to the share picker, shown from item [item]
/// ([MacToolbar.share]). Nothing is picked through it.
class ToolbarShareFiles implements BundleFiles {
  const ToolbarShareFiles({required this.view, this.item = 'share'});

  final int view;
  final String item;

  @override
  Future<bool> send(
    String name,
    Uint8List bytes, {
    required Rect origin,
  }) async {
    final dir = await Directory.systemTemp.createTemp('share');
    final file = File('${dir.path}/${name.replaceAll('/', '-')}');
    await file.writeAsBytes(bytes, flush: true);
    return MacToolbar.share(file.path, item: item, view: view);
  }

  @override
  Future<Uint8List?> pick() async => null;
}
