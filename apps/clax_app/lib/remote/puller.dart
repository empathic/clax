/// Getting an app from its public copy on a remote (spec
/// 2026-10-21-app-push-pull §5.5, §5.8, §5.13).
library;

import 'dart:typed_data';

import '../bundle/bundle_reader.dart';
import '../bundle/installer.dart';
import '../bundle/lineage_store.dart';
import '../bundle/trust.dart';
import '../clax/api.dart' show ClaxApiError;
import 'models.dart';
import 'remote_api.dart';
import 'remote_errors.dart';

/// What getting an app came to.
sealed class PullOutcome {
  const PullOutcome();
}

/// Installed as app [aid], linked to its public copy.
class PullInstalled extends PullOutcome {
  const PullInstalled(this.aid);

  final String aid;
}

/// App [aid] here holds this version already, or a later one.
class PullHave extends PullOutcome {
  const PullHave(this.aid);

  final String aid;
}

/// A linked app here takes this bundle as an update, which the update
/// sheet offers.
class PullUpdate extends PullOutcome {
  const PullUpdate(this.opened);

  final OpenedBundle opened;
}

/// The person did not install it.
class PullCancelled extends PullOutcome {
  const PullCancelled();
}

/// Refused, for the reason [message] gives in a sentence ([details]: the
/// raw refusal, for the log and Developer tools); nothing was written.
class PullRefused extends PullOutcome {
  const PullRefused(this.message, {this.details = const []});

  final String message;
  final List<String> details;
}

/// The sentence for an address that is not a public app's.
const notAnAppLink = 'That link isn’t a Clax app.';

/// Gets apps from public copies (§5.5).
///
/// A pull reads the copy's head (with this device's token when it is
/// signed in to the copy's remote, else without), then the bundle of the
/// latest version that carries one, and refuses it, with nothing written,
/// unless its manifest's content hash, lineage and version are the ones
/// the head names: the remote stores bundles unread, so the head (whose
/// hash the remote checked against the public version's files when the
/// bundle was put) is what vouches for them. It then opens the bundle as
/// from that copy ([Installer.openRead] with a [BundleOrigin]), so a linked
/// app here weighs it by version whatever key signed it.
///
/// A new app is installed after `confirm` (the install sheet) agrees,
/// linked to the copy from the start with its base at the version taken:
/// as an author when the signed-in account owns or may edit the copy,
/// else as a recipient; with only the copy's address when this device is
/// not signed in. The core links it too ([RemoteApi.link]) when signed
/// in, and the install is counted ([RemoteApi.recordInstall]) under
/// [installId]. Pulls never bring documents (§5.13): a seed the bundle
/// carries is left out.
class Puller {
  Puller({
    required this.remote,
    required this.installer,
    required this.store,
    required this.installId,
    void Function(String line)? log,
  }) : _log = log ?? ((_) {});

  final RemoteApi remote;
  final Installer installer;
  final LineageStore store;

  /// This install's ID for counting installs (`AppSettings.installId`).
  final Future<String> Function() installId;
  final void Function(String) _log;

  /// Gets the app at [artifactUrl] (`<origin>/a/<id>`, or its `/get`
  /// page). [confirm] answers whether to install a new app (the install
  /// sheet); without it the app installs.
  Future<PullOutcome> pull(
    Uri artifactUrl, {
    Future<bool> Function(OpenedBundle opened)? confirm,
  }) async {
    final at = publicAppUrl(artifactUrl);
    if (at == null) return const PullRefused(notAnAppLink);
    final (url, raid) = at;
    Remote? signedIn;
    try {
      for (final r in await remote.remotes()) {
        if (r.connected && _sameOrigin(r.origin, url)) signedIn = r;
      }
    } on ClaxApiError catch (e) {
      _log('the remotes could not be read: ${e.message}');
    }
    return _pull(url, raid, signedIn, null, confirm);
  }

  /// Gets [app] from the library of the account signed in to [rid].
  Future<PullOutcome> pullFromLibrary(
    String rid,
    RemoteApp app, {
    Future<bool> Function(OpenedBundle opened)? confirm,
  }) async {
    final Remote r;
    try {
      r = (await remote.remotes()).firstWhere(
        (r) => r.id == rid,
        orElse: () => throw ClaxApiError(
          status: 404,
          code: 'not_found',
          message: 'no remote $rid',
        ),
      );
    } on ClaxApiError catch (e) {
      return PullRefused(
        e.isNotFound
            ? 'This device is no longer signed in there.'
            : 'Couldn’t read where this device is signed in.',
        details: [e.message],
      );
    }
    return _pull(Uri.parse(app.url), app.artifactId, r, app, confirm);
  }

  Future<PullOutcome> _pull(
    Uri url,
    String raid,
    Remote? signedIn,
    RemoteApp? app,
    Future<bool> Function(OpenedBundle)? confirm,
  ) async {
    final host = url.host;
    final rid = signedIn?.id;
    final RemoteHead head;
    final Uint8List bytes;
    try {
      head = rid != null
          ? await remote.head(rid, raid)
          : await remote.anonymousHead(url);
      final b = head.bundle;
      if (b == null) return const PullRefused('This page isn’t an app yet.');
      bytes = rid != null
          ? await remote.bundle(rid, raid, b.version)
          : await remote.anonymousBundle(Uri.parse(b.url));
    } on ClaxApiError catch (e) {
      return PullRefused(
        remoteLine(e, host: host, action: 'share apps') ??
            switch (e) {
              ClaxApiError(status: 404 || 403) =>
                'This app isn’t on $host, or you can’t open it.',
              ClaxApiError(code: 'bundle_too_large') =>
                'This app is too large to get.',
              _ => remoteErrorLine(
                e,
                host: host,
                action: 'share apps',
                otherwise: 'Couldn’t get this app from $host.',
              ),
            },
        details: [e.message],
      );
    }
    final b = head.bundle!;
    final Bundle bundle;
    try {
      bundle = await BundleReader.read(bytes);
    } on BundleError catch (e) {
      return PullRefused(BundleFailure.fromError(e).message);
    }
    final m = bundle.manifest;
    if (m.contentSha256 != b.contentSha256 ||
        m.lineage != b.lineage ||
        m.version != b.bundleVersion) {
      return PullRefused(
        'This app’s file isn’t the version $host shows, so Clax didn’t '
        'open it.',
      );
    }
    final origin = BundleOrigin(
      remoteOrigin: _origin(url),
      remoteArtifactId: raid,
      remoteVersion: b.version,
      head: head,
    );
    final OpenedBundle opened;
    try {
      opened = await installer.openRead(_withoutSeed(bundle), origin: origin);
    } on BundleFailure catch (e) {
      return PullRefused(e.message, details: e.details);
    }
    final d = opened.decision;
    switch (d.answer) {
      case TrustAnswer.update:
        return PullUpdate(opened);
      case TrustAnswer.own ||
          TrustAnswer.current ||
          TrustAnswer.older ||
          TrustAnswer.conflict:
        return PullHave(d.aid!);
      case TrustAnswer.install || TrustAnswer.foreign || TrustAnswer.forked:
        break;
    }
    if (confirm != null && !await confirm(opened)) return const PullCancelled();
    final known = app ?? (rid == null ? null : await _libraryEntry(rid, raid));
    final link = AppLink(
      remoteOrigin: origin.remoteOrigin,
      remoteId: rid,
      remoteArtifactId: raid,
      url: head.url,
      baseRemoteVersion: b.version,
      takenBundleVersion: m.version,
      role: (known?.mayPublish ?? false) ? AppLink.author : AppLink.recipient,
    );
    final String aid;
    try {
      aid = (await installer.install(opened, link: link)).aid;
    } on BundleFailure catch (e) {
      return PullRefused(e.message, details: e.details);
    }
    // Who published it there, so a copy from the person's own account
    // (another of their devices) is known as theirs (`AuthorTrust`).
    await store.update(
      aid,
      (e) async => e?.copyWith(
        liveFrom: () => VersionSource(
          key: m.author.key,
          who: b.publishedBy.handle,
          origin: origin.remoteOrigin,
          account: b.publishedBy,
        ),
      ),
    );
    if (rid != null) {
      try {
        await remote.link(
          rid,
          aid,
          known ??
              RemoteApp(
                artifactId: raid,
                title: head.title,
                url: head.url,
                version: head.version,
                bundleVersion: b.bundleVersion,
                access: 'viewer',
                general: 'link',
              ),
          b.version,
        );
      } on ClaxApiError catch (e) {
        _log('the core could not link $aid to $raid: ${e.message}');
      }
    }
    try {
      // Signed in there, with the core's token, so a copy shared by name
      // counts; else without one, which counts only a copy shared by link.
      final id = await installId();
      if (rid != null) {
        await remote.countInstall(rid, raid, id);
      } else {
        await remote.recordInstall(Uri.parse(head.url), id);
      }
    } on ClaxApiError catch (e) {
      _log('the install of $raid was not counted: ${e.message}');
    }
    return PullInstalled(aid);
  }

  /// [raid] as the signed-in account's library lists it, or null.
  Future<RemoteApp?> _libraryEntry(String rid, String raid) async {
    try {
      for (final a in await remote.apps(rid)) {
        if (a.artifactId == raid) return a;
      }
    } on ClaxApiError catch (e) {
      _log('the library could not be read: ${e.message}');
    }
    return null;
  }

  static String _origin(Uri u) =>
      Uri(scheme: u.scheme, host: u.host, port: u.port).toString();

  static bool _sameOrigin(String origin, Uri url) {
    final o = Uri.parse(origin);
    return o.scheme == url.scheme && o.host == url.host && o.port == url.port;
  }

  /// [b] without its seed: a pull brings no documents (§5.13).
  static Bundle _withoutSeed(Bundle b) => b.seed == null
      ? b
      : Bundle(
          bytes: b.bytes,
          manifest: b.manifest,
          manifestBytes: b.manifestBytes,
          signature: b.signature,
          files: b.files,
          seed: null,
          iconSvg: b.iconSvg,
        );
}

/// The public app [url] names, as its address `<origin>/a/<id>` and the
/// artifact ID; its `/get` page names it too. Null for any other address.
(Uri, String)? publicAppUrl(Uri url) {
  final segs = [...url.pathSegments];
  if (segs.isNotEmpty && segs.last.isEmpty) segs.removeLast();
  if (segs.length == 3 && segs[2] == 'get') segs.removeLast();
  if ((url.scheme != 'https' && url.scheme != 'http') ||
      !url.hasAuthority ||
      segs.length != 2 ||
      segs[0] != 'a' ||
      segs[1].isEmpty) {
    return null;
  }
  return (
    Uri(
      scheme: url.scheme,
      host: url.host,
      port: url.port,
      pathSegments: ['a', segs[1]],
    ),
    segs[1],
  );
}
