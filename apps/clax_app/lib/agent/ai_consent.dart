/// The person's permission for Clax to send their requests to the Claude
/// API, asked once before the first run where the platform's store wants
/// it (App Review guideline 5.1.2(i), on iOS).
library;

import 'package:flutter/widgets.dart';

import '../platform/settings_store.dart';
import '../ui/ai_consent_sheet.dart';

/// Asks once, before a run sends anything to the Claude API, whether it may
/// ([showAiConsentSheet]), and keeps an Allow as [AppSettings.aiConsent].
class AiConsent {
  AiConsent({required this.required, required this.settings});

  /// Whether runs wait for the person's Allow; false lets every run go.
  final bool required;

  /// Where the answer is kept.
  final SettingsStore settings;

  /// The answer of the sheet showing, which every run that asks meanwhile
  /// shares.
  Future<bool>? _asking;

  /// Whether a run may send its requests: true at once when not [required]
  /// or once allowed; otherwise the sheet asks over [context], and Allow is
  /// kept. Not now, or closing the sheet, answers false and keeps nothing,
  /// so the next run asks again. A [context] no longer mounted answers
  /// false.
  Future<bool> ensure(BuildContext context) async {
    if (!required) return true;
    if ((await settings.load()).aiConsent == true) return true;
    if (!context.mounted) return false;
    return _asking ??= _ask(context).whenComplete(() => _asking = null);
  }

  Future<bool> _ask(BuildContext context) async {
    final allowed = await showAiConsentSheet(context) ?? false;
    if (allowed) {
      final s = await settings.load();
      s.aiConsent = true;
      await settings.save(s);
    }
    return allowed;
  }
}
