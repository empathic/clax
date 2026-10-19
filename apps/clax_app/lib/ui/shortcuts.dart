import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'kit/family.dart';

/// Clax's own commands: the menu bar's items on the Mac (`MainMenu.xib`,
/// which sends each as its [tag]) and the keys of [claxShortcuts].
enum ClaxCommand {
  newApp,
  importApp,
  shareApp,
  toggleSidebar,
  comment,
  toggleThreads,
  reload,
  actualSize,
  zoomIn,
  zoomOut,
  back,
  forward,
  home,
  inspect,
  versions,
  star,
  resumeBuild,
  inbox,
  settings,
  deleteApp,
  openSelected,
  starred1,
  starred2,
  starred3,
  starred4,
  starred5,
  starred6,
  starred7,
  starred8,
  starred9;

  /// The `tag` of this command's item in `MainMenu.xib`, from 1.
  int get tag => index + 1;

  /// The command whose item has [tag], or null for none.
  static ClaxCommand? ofTag(int tag) =>
      tag >= 1 && tag <= values.length ? values[tag - 1] : null;
}

/// [ClaxCommand] [command], as a key of [claxShortcuts] asks for it.
final class ClaxCommandIntent extends Intent {
  const ClaxCommandIntent(this.command);
  final ClaxCommand command;
}

/// The key for each command: Command on the Apple family, Control on
/// Material (a Chromebook's keyboard). Space, which opens the selected
/// app, takes no modifier in either.
///
/// On the Mac the menu bar's key equivalents are the same keys; these are
/// for windows where no menu bar delivers them.
Map<ShortcutActivator, Intent> claxShortcuts(ClaxFamily family) {
  final apple = family == ClaxFamily.apple;
  SingleActivator key(
    LogicalKeyboardKey trigger, {
    bool shift = false,
    bool alt = false,
    bool control = false,
  }) => SingleActivator(
    trigger,
    meta: apple,
    control: !apple || control,
    shift: shift,
    alt: alt,
  );
  const digits = [
    LogicalKeyboardKey.digit1,
    LogicalKeyboardKey.digit2,
    LogicalKeyboardKey.digit3,
    LogicalKeyboardKey.digit4,
    LogicalKeyboardKey.digit5,
    LogicalKeyboardKey.digit6,
    LogicalKeyboardKey.digit7,
    LogicalKeyboardKey.digit8,
    LogicalKeyboardKey.digit9,
  ];
  const starred = [
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
  return {
    key(LogicalKeyboardKey.keyN): const ClaxCommandIntent(ClaxCommand.newApp),
    key(LogicalKeyboardKey.keyO): const ClaxCommandIntent(
      ClaxCommand.importApp,
    ),
    key(LogicalKeyboardKey.keyS, control: true): const ClaxCommandIntent(
      ClaxCommand.toggleSidebar,
    ),
    key(LogicalKeyboardKey.keyK, shift: true): const ClaxCommandIntent(
      ClaxCommand.comment,
    ),
    key(LogicalKeyboardKey.keyK): const ClaxCommandIntent(
      ClaxCommand.toggleThreads,
    ),
    key(LogicalKeyboardKey.keyR): const ClaxCommandIntent(ClaxCommand.reload),
    key(LogicalKeyboardKey.digit0): const ClaxCommandIntent(
      ClaxCommand.actualSize,
    ),
    key(LogicalKeyboardKey.equal): const ClaxCommandIntent(
      ClaxCommand.zoomIn,
    ),
    key(LogicalKeyboardKey.minus): const ClaxCommandIntent(
      ClaxCommand.zoomOut,
    ),
    key(LogicalKeyboardKey.bracketLeft): const ClaxCommandIntent(
      ClaxCommand.back,
    ),
    key(LogicalKeyboardKey.bracketRight): const ClaxCommandIntent(
      ClaxCommand.forward,
    ),
    key(LogicalKeyboardKey.keyH, shift: true): const ClaxCommandIntent(
      ClaxCommand.home,
    ),
    key(LogicalKeyboardKey.keyI, alt: true): const ClaxCommandIntent(
      ClaxCommand.inspect,
    ),
    key(LogicalKeyboardKey.keyI, shift: true): const ClaxCommandIntent(
      ClaxCommand.inbox,
    ),
    key(LogicalKeyboardKey.comma): const ClaxCommandIntent(
      ClaxCommand.settings,
    ),
    key(LogicalKeyboardKey.backspace): const ClaxCommandIntent(
      ClaxCommand.deleteApp,
    ),
    const SingleActivator(LogicalKeyboardKey.space): const ClaxCommandIntent(
      ClaxCommand.openSelected,
    ),
    for (var i = 0; i < digits.length; i++)
      key(digits[i]): ClaxCommandIntent(starred[i]),
  };
}

/// The Mac's menu bar (`MainMenu.xib`, `macos/Runner/MenuBridge.swift`)
/// over the method channel `dev.empathic.clax/menu`: Clax's items arrive
/// as [commands], and their enabled state and titles are set from here.
/// Each call is a no-op unless [hasDesktopWindowChrome].
abstract final class MacMenu {
  static const channel = MethodChannel('dev.empathic.clax/menu');

  static final _commands = StreamController<ClaxCommand>.broadcast(
    onListen: _listen,
  );

  /// The items the person chose, from the first listener on.
  static Stream<ClaxCommand> get commands => _commands.stream;

  /// Takes the native side's `command` {tag}, answering whether the tag
  /// names a command.
  static void _listen() => channel.setMethodCallHandler((call) async {
    if (call.method != 'command') return false;
    final c = switch (call.arguments) {
      final int tag => ClaxCommand.ofTag(tag),
      _ => null,
    };
    if (c == null) return false;
    _commands.add(c);
    return true;
  });

  /// Enables or disables [c]'s menu item; items start disabled.
  static void setEnabled(ClaxCommand c, bool on) =>
      _send('setEnabled', {'tag': c.tag, 'enabled': on});

  /// Gives [c]'s menu item [title] in place of `MainMenu.xib`'s.
  static void setTitle(ClaxCommand c, String title) =>
      _send('setTitle', {'tag': c.tag, 'title': title});

  static void _send(String method, Map<String, Object> args) {
    if (!hasDesktopWindowChrome) return;
    unawaited(
      channel
          .invokeMethod<void>(method, args)
          .catchError((Object _) {}, test: (e) => e is MissingPluginException),
    );
  }
}
