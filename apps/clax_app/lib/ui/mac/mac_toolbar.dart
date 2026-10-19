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
/// sits before the window's title. A [search] item is a search field
/// (`NSSearchToolbarItem`) whose placeholder is [label]; its text arrives
/// on [MacToolbar.searches], and what was typed stays while the item does.
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
    this.search = false,
  });

  final String id, symbol, label;
  final bool enabled, on, search;
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
    if (search) 'search': true,
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
      other.group == group &&
      other.search == search;

  @override
  int get hashCode =>
      Object.hash(id, symbol, label, enabled, on, badge, tint, group, search);

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

/// The text of search item [id] of [view]'s window's toolbar changed to
/// [text].
@immutable
final class ToolbarSearch {
  const ToolbarSearch(this.view, this.id, this.text);

  final int view;
  final String id, text;

  @override
  bool operator ==(Object other) =>
      other is ToolbarSearch &&
      other.view == view &&
      other.id == id &&
      other.text == text;

  @override
  int get hashCode => Object.hash(view, id, text);

  @override
  String toString() => 'ToolbarSearch($view, $id, $text)';
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
/// - `setItems` {view, items: [ToolbarItemSpec.toMap…], leading?, edge?}:
///   the window's items, in order; none hides the title. With `edge` the
///   first `leading` items sit over a sidebar `edge` points wide from the
///   window's leading edge, and the rest (Back, Forward, the title) start
///   past it.
/// - `share` {view, item, path}: the share picker for the file at [path],
///   from item [item]; answers whether a target was chosen.
/// - `focusSearch` {view, id}: the keyboard goes to search item `id`;
///   answers whether the window has it.
/// - From the native side, `clicked` {view, id}: arrives on [clicks]; and
///   `search` {view, id, text}, a search item's text as it changes: on
///   [searches].
abstract final class MacToolbar {
  static const channel = MethodChannel('dev.empathic.clax/toolbar');

  static final _clicks = StreamController<ToolbarClick>.broadcast();
  static final _searches = StreamController<ToolbarSearch>.broadcast();
  static final _owners = <int, Object>{};
  static final _declared = <int, List<ToolbarItemSpec>>{};
  static final _leading = <int, List<ToolbarItemSpec>>{};
  static final _edge = <int, double>{};
  static final _sent = <int, (List<ToolbarItemSpec>, double)>{};
  static var _listening = false;

  /// The items people click, in any window.
  static Stream<ToolbarClick> get clicks {
    _listen();
    return _clicks.stream;
  }

  /// The text of search items as people type, in any window.
  static Stream<ToolbarSearch> get searches {
    _listen();
    return _searches.stream;
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
  /// ([setItems]); none while no owner declares any. With an [edge] above
  /// 0 they sit over a sidebar that wide, and the owner's items start
  /// past it.
  static void setLeading(
    List<ToolbarItemSpec> items, {
    required int view,
    double edge = 0,
  }) {
    if (!hasDesktopWindowChrome) return;
    _listen();
    _leading[view] = List.of(items);
    _edge[view] = edge;
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
    final leading = own.isEmpty
        ? const <ToolbarItemSpec>[]
        : _leading[view] ?? const <ToolbarItemSpec>[];
    final items = [...leading, ...own];
    final edge = leading.isEmpty ? 0.0 : _edge[view] ?? 0.0;
    final last = _sent[view];
    if (last != null && last.$2 == edge && listEquals(last.$1, items)) return;
    _sent[view] = (List.of(items), edge);
    unawaited(
      _invoke<void>('setItems', {
        'view': view,
        'items': [for (final i in items) i.toMap()],
        if (edge > 0) ...{'leading': leading.length, 'edge': edge},
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

  /// Puts the keyboard in search item [id] of [view]'s window's toolbar;
  /// whether the window has that item.
  static Future<bool> focusSearch(String id, {required int view}) async {
    if (!hasDesktopWindowChrome) return false;
    return await _invoke<bool>('focusSearch', {'view': view, 'id': id}) ??
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
      if (call.method == 'search') {
        final search = switch (call.arguments) {
          {
            'view': final int view,
            'id': final String id,
            'text': final String text,
          } =>
            ToolbarSearch(view, id, text),
          _ => null,
        };
        if (search == null) return false;
        _searches.add(search);
        return true;
      }
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
    _edge.clear();
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
