import 'package:flutter/material.dart';

import '../../ui/kit/kit.dart';
import 'device_grants.dart';
import 'device_host.dart';

/// [DeviceConsent] in a `ClaxDialog` over the app: "Let “Plant care” send
/// you notifications and reminders?" with Don’t allow and **Allow**; for
/// location, Precise (chosen at first) and Approximate as radios in the
/// body. Dismissing it is no answer ([DeviceAnswer.dismissed]: kept
/// nowhere, the next call asks again); so is having no navigator (the app
/// not up yet).
class DialogDeviceConsent implements DeviceConsent {
  DialogDeviceConsent({required this.navigator, required this.title});

  final NavigatorState? Function() navigator;

  /// App [aid]'s title, for the question; empty when unknown.
  final Future<String> Function(String aid) title;

  static const _keep =
      'It keeps this until you take it back in the app inspector.';

  @override
  Future<DeviceAnswer> ask(String aid, String cap) async {
    String name;
    try {
      name = await title(aid);
    } on Object {
      name = '';
    }
    final nav = navigator();
    if (nav == null || !nav.mounted) return DeviceAnswer.dismissed;
    final who = name.trim().isEmpty ? 'this app' : '“${name.trim()}”';
    final location = cap == 'location';
    final precise = ValueNotifier(true);
    final got = await ClaxDialog.ask(
      nav.context,
      title: 'Let $who ${deviceCapWords[cap] ?? 'use $cap'}?',
      body: _keep,
      content: location ? _Precision(precise) : null,
      actions: const [
        ClaxDialogAction('Don’t allow'),
        ClaxDialogAction('Allow', kind: ClaxButtonKind.primary),
      ],
      defaultIndex: 1,
    );
    final exact = precise.value;
    WidgetsBinding.instance.addPostFrameCallback((_) => precise.dispose());
    return switch (got.index) {
      0 => DeviceAnswer.deny,
      1 when location && !exact => DeviceAnswer.approximate,
      1 => DeviceAnswer.allow,
      _ => DeviceAnswer.dismissed,
    };
  }
}

/// Location's two precisions, each saying how exactly the app would see
/// where the device is.
class _Precision extends StatelessWidget {
  const _Precision(this.precise);
  final ValueNotifier<bool> precise;

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: precise,
    builder: (context, exact, _) => Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        ClaxRadio<bool>(
          key: const ValueKey('consent-precise'),
          value: true,
          group: exact,
          onChanged: (v) => precise.value = v,
          label: 'Precise',
          detail: 'Your exact location, within a few metres',
        ),
        ClaxRadio<bool>(
          key: const ValueKey('consent-approximate'),
          value: false,
          group: exact,
          onChanged: (v) => precise.value = v,
          label: 'Approximate',
          detail: 'Only your area, within about 2 km',
        ),
      ],
    ),
  );
}
