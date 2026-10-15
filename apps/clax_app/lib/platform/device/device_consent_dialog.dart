import 'dart:async';

import 'package:flutter/material.dart';

import 'device_grants.dart';
import 'device_host.dart';

/// [DeviceConsent] in a dialog over the app: "Let “Plant care” send you
/// notifications and reminders?" with Allow and Don't allow; for location,
/// Precise, Approximate and Don't allow. Dismissing it is Don't allow; so
/// is having no navigator (the app not up yet).
class DialogDeviceConsent implements DeviceConsent {
  DialogDeviceConsent({required this.navigator, required this.title});

  final NavigatorState? Function() navigator;

  /// App [aid]'s title, for the question; empty when unknown.
  final Future<String> Function(String aid) title;

  @override
  Future<DeviceAnswer> ask(String aid, String cap) async {
    String name;
    try {
      name = await title(aid);
    } on Object {
      name = '';
    }
    final nav = navigator();
    if (nav == null || !nav.mounted) return DeviceAnswer.deny;
    final answer = await nav.push<DeviceAnswer>(
      DialogRoute<DeviceAnswer>(
        context: nav.context,
        builder: (_) => DeviceConsentDialog(app: name, cap: cap),
      ),
    );
    return answer ?? DeviceAnswer.deny;
  }
}

/// The question itself; pops with the [DeviceAnswer].
class DeviceConsentDialog extends StatelessWidget {
  const DeviceConsentDialog({super.key, required this.app, required this.cap});

  final String app;
  final String cap;

  static const _keep =
      'It keeps this until you take it back in the app inspector.';

  @override
  Widget build(BuildContext context) {
    final who = app.trim().isEmpty ? 'this app' : '“${app.trim()}”';
    final title = Text('Let $who ${deviceCapWords[cap] ?? 'use $cap'}?');
    void answer(DeviceAnswer a) => Navigator.of(context).pop(a);
    if (cap == 'location') return _location(context, title, answer);
    return AlertDialog(
      title: title,
      content: const Text(_keep),
      actions: [
        TextButton(
          onPressed: () => answer(DeviceAnswer.deny),
          child: const Text("Don't allow"),
        ),
        FilledButton(
          onPressed: () => answer(DeviceAnswer.allow),
          child: const Text('Allow'),
        ),
      ],
    );
  }

  /// Location's three answers, one under the other, each saying how
  /// exactly the app would see where the device is.
  Widget _location(
    BuildContext context,
    Widget title,
    void Function(DeviceAnswer) answer,
  ) {
    final text = Theme.of(context).textTheme;
    Widget choice({
      required Key key,
      required IconData icon,
      required String label,
      required String detail,
      required DeviceAnswer value,
      bool filled = false,
    }) {
      final child = Row(
        children: [
          Icon(icon, size: 22),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(label, style: text.labelLarge),
                Text(detail, style: text.bodySmall),
              ],
            ),
          ),
        ],
      );
      final style = ButtonStyle(
        alignment: Alignment.centerLeft,
        shape: const WidgetStatePropertyAll(
          RoundedRectangleBorder(
            borderRadius: BorderRadius.all(Radius.circular(16)),
          ),
        ),
        padding: const WidgetStatePropertyAll(
          EdgeInsets.symmetric(horizontal: 16, vertical: 10),
        ),
        foregroundColor: filled
            ? null
            : WidgetStatePropertyAll(Theme.of(context).colorScheme.onSurface),
      );
      return Padding(
        padding: const EdgeInsets.only(top: 8),
        child: filled
            ? FilledButton.tonal(
                key: key,
                style: style,
                onPressed: () => answer(value),
                child: child,
              )
            : OutlinedButton(
                key: key,
                style: style,
                onPressed: () => answer(value),
                child: child,
              ),
      );
    }

    return AlertDialog(
      title: title,
      content: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          choice(
            key: const ValueKey('consent-precise'),
            icon: Icons.my_location_rounded,
            label: 'Precise',
            detail: 'Your exact location, within a few metres',
            value: DeviceAnswer.allow,
            filled: true,
          ),
          choice(
            key: const ValueKey('consent-approximate'),
            icon: Icons.location_searching_rounded,
            label: 'Approximate',
            detail: 'Only your area, within about 2 km',
            value: DeviceAnswer.approximate,
          ),
          choice(
            key: const ValueKey('consent-deny'),
            icon: Icons.location_disabled_rounded,
            label: "Don't allow",
            detail: 'The app gets no location',
            value: DeviceAnswer.deny,
          ),
          const SizedBox(height: 12),
          Text(_keep, style: text.bodySmall),
        ],
      ),
    );
  }
}
