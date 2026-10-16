/// Putting a broken home app right: Undo last change and Reset to Clax's
/// home screen.
library;

import 'package:flutter/foundation.dart';

import '../bundle/installer.dart';
import '../bundle/lineage_store.dart';
import '../clax/api.dart';
import 'home_install.dart';
import 'home_settings.dart';

/// Why Undo or Reset could not run, in one sentence for the person.
class HomeRecoveryError implements Exception {
  const HomeRecoveryError(this.message);
  final String message;
  @override
  String toString() => 'HomeRecoveryError: $message';
}

/// Restores the version of home app [homeAppId] before its latest
/// (`restoreVersion(n - 1)`, as the Versions tab's Undo does): its files
/// as a new version, documents and capabilities untouched. Throws
/// [HomeRecoveryError] when it has no earlier version, and what [api]
/// throws (409 `conflict` when a version landed meanwhile).
Future<PublishResult> undoLast(ClaxApi api, String homeAppId) async {
  final n = (await api.getArtifact(homeAppId)).artifact.currentVersion;
  if (n < 2) {
    throw const HomeRecoveryError('Your home screen has no earlier version.');
  }
  return api.restoreVersion(homeAppId, n - 1, ifVersion: n);
}

/// Publishes the shipped home bundle's files ([shipped], the app's assets
/// by default) as a new version of home app [homeAppId], with the
/// bundle's title, icon and capabilities. Its documents and threads stay;
/// files the bundle lacks are removed. The bundle must be the one
/// `release.json` names, as for [ensureHomeApp].
///
/// The app's lineage entry then records the shipped release as its latest
/// ([LineageEntry.withRelease], updates no longer stopped), with the
/// bundle kept, so the update check reads the app as that release,
/// unmodified; an app with no entry gets the one [Installer.install]
/// writes. [settings]' [HomeSettings.keptRelease] is cleared. Throws
/// [HomeInstallError] for a mismatched bundle, [HomeRecoveryError] when
/// the published files are not the bundle's, and what [Installer.open]
/// and [api] throw.
Future<PublishResult> reset(
  Installer installer,
  ClaxApi api,
  String homeAppId, {
  ShippedHome? shipped,
  HomeSettings? settings,
  Future<void> Function()? ensureSession,
}) async {
  final s = shipped ?? AssetShippedHome();
  final release = await s.release();
  final opened = await installer.open(await s.bundle());
  final m = opened.manifest;
  final mismatch = [
    if (m.author.key != release.key) 'author key',
    if (m.lineage != release.lineage) 'lineage',
    if (m.version != release.version) 'version',
    if (m.contentSha256 != release.hash) 'content hash',
  ];
  if (mismatch.isNotEmpty) {
    throw HomeInstallError(
      'the shipped home bundle differs from release.json in its '
      '${mismatch.join(', ')}',
    );
  }
  final b = opened.bundle;
  final live = await api.getArtifact(homeAppId);
  await ensureSession?.call();
  final published = await api.publish(
    homeAppId,
    PublishRequest(
      ifVersion: live.artifact.currentVersion,
      title: m.title,
      icon: m.icon.word,
      capabilities: m.capabilities,
      label: 'Reset to v${m.version}',
      note: "Reset to Clax's home screen v${m.version}",
      entries: {
        for (final MapEntry(:key, :value) in m.files.entries)
          key: FileEntry.bytes(b.files[key]!, type: value.type),
        for (final p in live.current?.files.keys ?? const <String>[])
          if (!m.files.containsKey(p)) p: null,
      },
    ),
  );
  final got = published.version.contentSha256;
  if (got != m.contentSha256) {
    throw HomeRecoveryError(
      'The reset home screen is $got, not the shipped ${m.contentSha256}.',
    );
  }
  final state = ReleaseState(
    contentSha256: m.contentSha256,
    title: m.title,
    capabilities: m.capabilities,
  );
  await installer.store.update(
    homeAppId,
    (now) async =>
        now
            ?.withRelease(state, version: m.version)
            .copyWith(
              name: () => m.author.name,
              updatesStopped: false,
              stoppedAt: () => null,
            ) ??
        LineageEntry(
          lineage: m.lineage,
          role: LineageRole.installed,
          key: m.author.key,
          name: m.author.name,
          version: m.version,
          released: [m.contentSha256],
          capabilities: m.capabilities,
          title: m.title,
          fromBundle: true,
        ),
  );
  await installer.store.keepBundle(homeAppId, b.bytes);
  if (settings != null && settings.keptRelease != null) {
    settings.keptRelease = null;
    await settings.save();
  }
  return published;
}

/// Undo and Reset for the recorded home app, shared by the fallback's
/// banner and Settings › Home screen. Listeners hear each that lands, so
/// the home screen loads the home app again.
class HomeRecovery extends ChangeNotifier {
  HomeRecovery({
    required this.api,
    required this.installer,
    required this.settings,
    this.shipped,
    this.ensureSession,
  });

  final ClaxApi api;
  final Installer installer;
  final HomeSettings settings;
  final ShippedHome? shipped;
  final Future<void> Function()? ensureSession;

  String get _aid =>
      settings.homeAppId ??
      (throw const HomeRecoveryError('Clax has no home screen app yet.'));

  /// [undoLast] on the recorded home app.
  Future<void> undoLast() async {
    await ensureSession?.call();
    await _undo(api, _aid);
    notifyListeners();
  }

  /// [reset] on the recorded home app.
  Future<void> reset() async {
    await _reset(
      installer,
      api,
      _aid,
      shipped: shipped,
      settings: settings,
      ensureSession: ensureSession,
    );
    notifyListeners();
  }
}

const _undo = undoLast;
const _reset = reset;

/// [e] as the person reads it after Undo or Reset failed.
String recoveryFailure(Object e) => switch (e) {
  HomeRecoveryError(:final message) => message,
  ClaxApiError(isConflict: true) =>
    'Your home screen changed meanwhile. Try again.',
  ClaxApiError(:final message) => message,
  BundleFailure(:final message) => message,
  _ => '$e',
};
