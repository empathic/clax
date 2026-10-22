import 'package:flutter/widgets.dart';

/// Whether Clax shows its developer views below this point: recent
/// timings, raw JSON, MIME types, capability names, the planned
/// permission kinds ("Coming") and raw error text. Settings' "Developer
/// tools" switch (`AppSettings.developer`) sets [notifier]; the views
/// read it with `if (DeveloperScope.of(context)) …` and rebuild as it
/// changes.
class DeveloperScope extends InheritedNotifier<ValueNotifier<bool>> {
  const DeveloperScope({
    super.key,
    required ValueNotifier<bool> notifier,
    required super.child,
  }) : super(notifier: notifier);

  /// Whether developer views show; false with no [DeveloperScope] above
  /// [context]. Registers [context] to rebuild when it changes.
  static bool of(BuildContext context) =>
      context
          .dependOnInheritedWidgetOfExactType<DeveloperScope>()
          ?.notifier
          ?.value ??
      false;

  /// Shows or hides the developer views for everything under the nearest
  /// [DeveloperScope] at once; does nothing without one. Saving the
  /// choice is the caller's.
  static void set(BuildContext context, bool on) {
    final n = context.getInheritedWidgetOfExactType<DeveloperScope>()?.notifier;
    if (n != null) n.value = on;
  }
}
