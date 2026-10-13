import 'dart:async';

import 'package:flutter/material.dart';

import 'device_grants.dart';
import 'device_host.dart';

/// [DeviceConsent] in a dialog over the app: "Let “Plant care” send you
/// notifications and reminders?" with Allow and Don't allow. Dismissing it
/// is Don't allow; so is having no navigator (the app not up yet).
class DialogDeviceConsent implements DeviceConsent {
  DialogDeviceConsent({required this.navigator, required this.title});

  final NavigatorState? Function() navigator;

  /// App [aid]'s title, for the question; empty when unknown.
  final Future<String> Function(String aid) title;

  @override
  Future<bool> ask(String aid, String cap) async {
    String name;
    try {
      name = await title(aid);
    } on Object {
      name = '';
    }
    final nav = navigator();
    if (nav == null || !nav.mounted) return false;
    final ok = await nav.push<bool>(
      DialogRoute<bool>(
        context: nav.context,
        builder: (_) => DeviceConsentDialog(app: name, cap: cap),
      ),
    );
    return ok ?? false;
  }
}

/// The question itself; pops with true for Allow.
class DeviceConsentDialog extends StatelessWidget {
  const DeviceConsentDialog({super.key, required this.app, required this.cap});

  final String app;
  final String cap;

  @override
  Widget build(BuildContext context) {
    final who = app.trim().isEmpty ? 'this app' : '“${app.trim()}”';
    return AlertDialog(
      title: Text('Let $who ${deviceCapWords[cap] ?? 'use $cap'}?'),
      content: const Text(
        'It keeps this until you take it back in the app inspector.',
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(false),
          child: const Text("Don't allow"),
        ),
        FilledButton(
          onPressed: () => Navigator.of(context).pop(true),
          child: const Text('Allow'),
        ),
      ],
    );
  }
}
