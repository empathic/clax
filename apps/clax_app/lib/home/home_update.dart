/// Updating the installed home app from the bundle a Clax release ships
/// (`assets/home/home.clax`), at startup.
library;

import 'package:flutter/widgets.dart';

import '../bundle/bundle_reader.dart';
import '../bundle/installer.dart';
import '../bundle/trust.dart';
import '../bundle/updater.dart';
import '../ui/bundle_sheets.dart';
import 'home_install.dart';
import 'home_settings.dart';

/// What [updateHomeApp] did.
enum HomeUpdateResult {
  /// Nothing to do: no home app, the shipped release is installed (or
  /// older), or the person closed the sheet without answering.
  none,

  /// The shipped release landed.
  updated,

  /// The person keeps their own home app over this release.
  kept,

  /// The update's check failed or it was refused; the home app is as it
  /// was, and the next start tries again.
  failed,
}

/// Runs an update of app `aid` to `bundle` ([Updater.update]).
typedef HomeUpdate = Future<UpdateOutcome> Function(String aid, Bundle bundle);

/// Asks the person whether to replace their changed home app with
/// `opened`; null when they answered nothing.
typedef HomeUpdateAsk = Future<UpdateChoice?> Function(OpenedBundle opened);

/// Brings the home app ([HomeSettings.homeAppId]) to the release [shipped]
/// carries, by the bundle trust rules ([decide]): a later version of the
/// home app's lineage and key updates it through [update], silently when
/// the home app is unmodified (its live content hash and capabilities are
/// a shipped release's), else only when [ask] answers
/// [UpdateChoice.update]. [UpdateChoice.keepMine] saves the release in
/// [HomeSettings.keptRelease], and that release is not asked about again;
/// a later one is. The person's documents stay either way.
///
/// The bundle must be the one its release record names ([checkShipped]),
/// else [HomeInstallError]. Throws what [Installer.open] throws.
Future<HomeUpdateResult> updateHomeApp(
  HomeSettings settings,
  Installer installer, {
  required ShippedHome shipped,
  required HomeUpdate update,
  required HomeUpdateAsk ask,
}) async {
  final aid = settings.homeAppId;
  if (aid == null) return HomeUpdateResult.none;
  final release = await shipped.release();
  if (settings.keptRelease == release.release) return HomeUpdateResult.kept;
  final opened = await installer.open(await shipped.bundle());
  checkShipped(opened.manifest, release);
  final d = opened.decision;
  if (d.answer != TrustAnswer.update || d.aid != aid) {
    return HomeUpdateResult.none;
  }
  if (d.modified) {
    switch (await ask(opened)) {
      case null:
        return HomeUpdateResult.none;
      case UpdateChoice.keepMine:
        settings.keptRelease = release.release;
        await settings.save();
        return HomeUpdateResult.kept;
      case UpdateChoice.update:
        break;
    }
  }
  final out = await update(aid, opened.bundle);
  if (out is! UpdateLanded) return HomeUpdateResult.failed;
  if (settings.keptRelease != null) {
    settings.keptRelease = null;
    await settings.save();
  }
  return HomeUpdateResult.updated;
}

/// The update sheet for a changed home app: "Keep mine" or "Update
/// (replaces your changes)".
Future<UpdateChoice?> askHomeUpdate(BuildContext context, OpenedBundle opened) {
  final d = opened.decision;
  return showBundleSheet<UpdateChoice>(
    context,
    UpdateSheet(
      bundle: BundleSummary.of(opened),
      have: d.entry?.version ?? 0,
      added: addedCapabilityWords(
        addedCapabilities(
          opened.live?.capabilities ?? d.entry?.capabilities ?? const {},
          opened.manifest.capabilities,
        ),
      ),
      modified: true,
    ),
  );
}
