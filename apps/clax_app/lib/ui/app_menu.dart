import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../home/home_host.dart';
import 'kit/family.dart';
import 'shortcuts.dart';

/// Esc on macOS, as the app maps it ([escapeShortcuts]).
class EscapeIntent extends Intent {
  const EscapeIntent();
}

/// The app's shortcuts on macOS: the platform's, with Esc as
/// [EscapeIntent] in place of dismissing.
Map<ShortcutActivator, Intent> escapeShortcuts() => {
  ...WidgetsApp.defaultShortcuts,
  const SingleActivator(LogicalKeyboardKey.escape): const EscapeIntent(),
};

/// The app's actions on macOS: the platform's, with [EscapeIntent] running
/// [escapeOrBack] with [back].
Map<Type, Action<Intent>> escapeActions(VoidCallback back) => {
  ...WidgetsApp.defaultActions,
  EscapeIntent: CallbackAction<EscapeIntent>(
    onInvoke: (_) {
      escapeOrBack(back);
      return null;
    },
  ),
};

/// Esc: a focused text field loses the focus (its text stays); otherwise
/// [back] runs. Esc never goes back from a field being typed in.
void escapeOrBack(VoidCallback back) {
  final focus = FocusManager.instance.primaryFocus;
  final context = focus?.context;
  if (focus != null &&
      context != null &&
      (context.widget is EditableText ||
          context.findAncestorWidgetOfExactType<EditableText>() != null)) {
    focus.unfocus();
    return;
  }
  back();
}

/// What View › Show Threads (⌘K) and View › Comment (⇧⌘K) run: the open app
/// screen registers its panel toggle and its comment-mode toggle here, and
/// the menu items are disabled while none is registered.
///
/// Listeners hear of a change in a microtask, never synchronously, because
/// the app screen registers while it is being built and unregisters while
/// it is being disposed, when the menu bar above it cannot rebuild.
class CommentsCommand extends ChangeNotifier {
  VoidCallback? _toggle;
  VoidCallback? _comment;
  var _disposed = false;

  /// The registered panel toggle (Threads), or null.
  VoidCallback? get toggle => _toggle;

  /// The registered comment-mode toggle (Comment), or null.
  VoidCallback? get comment => _comment;

  /// Makes [toggle] and [comment] the commands, replacing any others.
  void register(VoidCallback toggle, {VoidCallback? comment}) {
    _toggle = toggle;
    _comment = comment;
    _notifyLater();
  }

  /// Clears the commands when [toggle] is still registered.
  void unregister(VoidCallback toggle) {
    if (_toggle != toggle) return;
    _toggle = null;
    _comment = null;
    _notifyLater();
  }

  void _notifyLater() => scheduleMicrotask(() {
    if (!_disposed) notifyListeners();
  });

  @override
  void dispose() {
    _disposed = true;
    super.dispose();
  }
}

/// Clax's commands on the Mac, whose menu bar is `MainMenu.xib`'s
/// (Clax, File, Edit, View, App, Window, Help): it enables the items of
/// Clax's own commands and runs each the person chooses ([MacMenu]), and
/// takes Edit's items for the focused Flutter field ([MacEdit]). Edit's
/// items go to a page's WebView natively. Off the Mac it only gives
/// [ClaxCommandIntent] the same commands.
///
/// - Settings… (⌘,) runs [settings].
/// - New App… (⌘N), Import App… (⌘O) and Inbox (⇧⌘I) run [home]'s flows.
/// - Comment (⇧⌘K) and Show Threads (⌘K) run what the open app screen
///   registered with [comments].
/// - Back (⌘[) goes back as the platform's back would: the open app's page
///   steps back in its own history first (its `PopScope`), then the screen
///   closes. Home (⇧⌘H) closes every screen down to the grid.
/// - Show Sidebar or Hide Sidebar (⌃⌘S) runs [toggleSidebar], while a
///   split window (`SplitShell`) has a sidebar to toggle.
/// - Reload App (⌘R) runs [reload]: the home screen's Refresh while it is
///   what the window shows.
///
/// The other commands' items stay disabled. Key equivalents are the menu's,
/// so they work while a page's WebView has the keyboard focus; Esc is the
/// app's ([escapeShortcuts], [escapeActions], given to the MaterialApp).
class ClaxMenuBar extends StatefulWidget {
  const ClaxMenuBar({
    super.key,
    required this.comments,
    required this.child,
    this.navigator,
    this.home,
    this.settings,
    this.toggleSidebar,
    this.reload,
    this.titles = const {},
  });

  final CommentsCommand comments;
  final Widget child;

  /// The navigator Back and Home act on; without it, the one above this
  /// widget, if any.
  final NavigatorState? Function()? navigator;

  /// The home screen's flows, once there is a home screen.
  final HomeActions? home;

  /// Shows Settings; null disables Settings….
  final VoidCallback? settings;

  /// Hides or shows the split window's sidebar; null disables the item.
  final VoidCallback? toggleSidebar;

  /// What View › Reload App runs; null disables the item.
  final VoidCallback? reload;

  /// Titles in place of `MainMenu.xib`'s.
  final Map<ClaxCommand, String> titles;

  /// The commands with an item in the menu bar.
  static const menuCommands = [
    ClaxCommand.settings,
    ClaxCommand.newApp,
    ClaxCommand.importApp,
    ClaxCommand.shareApp,
    ClaxCommand.toggleSidebar,
    ClaxCommand.comment,
    ClaxCommand.toggleThreads,
    ClaxCommand.reload,
    ClaxCommand.actualSize,
    ClaxCommand.zoomIn,
    ClaxCommand.zoomOut,
    ClaxCommand.back,
    ClaxCommand.forward,
    ClaxCommand.home,
    ClaxCommand.inspect,
    ClaxCommand.versions,
    ClaxCommand.star,
    ClaxCommand.resumeBuild,
    ClaxCommand.inbox,
  ];

  @override
  State<ClaxMenuBar> createState() => _ClaxMenuBarState();
}

class _ClaxMenuBarState extends State<ClaxMenuBar> {
  StreamSubscription<MenuChoice>? _chosen;

  /// What was last sent, so a state is sent only when it changes.
  final _enabled = <ClaxCommand, bool>{};
  final _titles = <ClaxCommand, String>{};

  @override
  void initState() {
    super.initState();
    widget.comments.addListener(_sync);
    if (hasDesktopWindowChrome) {
      _chosen = MacMenu.choices.listen((c) {
        // A choice made with another window main is that window's.
        if (c.view == null || c.view == _view) _run(c.command);
      });
      MacEdit.install();
    }
    WidgetsBinding.instance.addPostFrameCallback((_) => _sync());
  }

  @override
  void didUpdateWidget(ClaxMenuBar old) {
    super.didUpdateWidget(old);
    if (old.comments != widget.comments) {
      old.comments.removeListener(_sync);
      widget.comments.addListener(_sync);
    }
    WidgetsBinding.instance.addPostFrameCallback((_) => _sync());
  }

  @override
  void dispose() {
    widget.comments.removeListener(_sync);
    unawaited(_chosen?.cancel());
    super.dispose();
  }

  NavigatorState? get _navigator {
    if (widget.navigator case final nav?) return nav();
    return mounted ? Navigator.maybeOf(context) : null;
  }

  bool get _hasNavigator =>
      widget.navigator != null || Navigator.maybeOf(context) != null;

  /// What [c] runs now, or null when it is disabled.
  VoidCallback? _handler(ClaxCommand c) => switch (c) {
    ClaxCommand.settings => widget.settings,
    ClaxCommand.newApp => widget.home?.newApp,
    ClaxCommand.importApp => widget.home?.importApp,
    ClaxCommand.inbox => widget.home?.openInbox,
    ClaxCommand.toggleSidebar => widget.toggleSidebar,
    ClaxCommand.reload => widget.reload,
    ClaxCommand.comment => widget.comments.comment,
    ClaxCommand.toggleThreads => widget.comments.toggle,
    ClaxCommand.back => _hasNavigator ? _back : null,
    ClaxCommand.home => _hasNavigator ? _home : null,
    _ => null,
  };

  /// The view this menu bar's window shows, once built.
  int? get _view => mounted ? View.maybeOf(context)?.viewId : null;

  void _back() => unawaited(_navigator?.maybePop());

  void _home() => _navigator?.popUntil((r) => r.isFirst);

  void _run(ClaxCommand c) {
    if (mounted) _handler(c)?.call();
  }

  void _sync() {
    if (!mounted || !hasDesktopWindowChrome) return;
    for (final c in ClaxMenuBar.menuCommands) {
      final on = _handler(c) != null;
      if (_enabled[c] != on) MacMenu.setEnabled(c, on, view: _view);
      _enabled[c] = on;
    }
    for (final MapEntry(key: c, value: title) in widget.titles.entries) {
      if (_titles[c] != title) MacMenu.setTitle(c, title);
      _titles[c] = title;
    }
  }

  @override
  Widget build(BuildContext context) => Actions(
    actions: {
      ClaxCommandIntent: CallbackAction<ClaxCommandIntent>(
        onInvoke: (i) {
          _run(i.command);
          return null;
        },
      ),
    },
    child: widget.child,
  );
}

/// Edit's items on the Mac while the Flutter view has the keyboard focus:
/// `macos/Runner/EditBridge.swift` sends `cut`, `copy`, `paste`,
/// `pasteAsPlainText`, `delete`, `selectAll`, `undo` or `redo` {view} over
/// the method channel `dev.empathic.clax/edit`, with the view of the key
/// window, and the focused field (or selectable text) takes it as the
/// matching text intent when it is in that view. Each answers whether
/// something took it. A page's WebView gets them natively.
abstract final class MacEdit {
  static const channel = MethodChannel('dev.empathic.clax/edit');

  /// Takes the channel's calls; called again, it changes nothing.
  static void install() => channel.setMethodCallHandler(
    (call) async => perform(
      call.method,
      view: switch (call.arguments) {
        {'view': final int v} => v,
        _ => null,
      },
    ),
  );

  /// Runs Edit's [action] on the primary focus, when it is in [view] (any
  /// view, when null); whether it was taken.
  static bool perform(String action, {int? view}) {
    const cause = SelectionChangedCause.keyboard;
    final Intent? intent = switch (action) {
      'cut' => const CopySelectionTextIntent.cut(cause),
      'copy' => CopySelectionTextIntent.copy,
      'paste' || 'pasteAsPlainText' => const PasteTextIntent(cause),
      'delete' => const DeleteCharacterIntent(forward: true),
      'selectAll' => const SelectAllTextIntent(cause),
      'undo' => const UndoTextIntent(cause),
      'redo' => const RedoTextIntent(cause),
      _ => null,
    };
    final context = FocusManager.instance.primaryFocus?.context;
    if (intent == null || context == null) return false;
    if (view != null && View.maybeOf(context)?.viewId != view) return false;
    final found = Actions.maybeFind<Intent>(context, intent: intent);
    if (found == null || !found.isEnabled(intent)) return false;
    Actions.invoke(context, intent);
    return true;
  }
}
