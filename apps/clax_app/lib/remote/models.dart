/// The records of a hosted Clax the app publishes apps to and gets them
/// from (a remote), as the core's remote routes and the remote's own
/// public routes answer them.
///
/// Each `fromJson` reads snake_case keys and throws a [TypeError] or
/// [FormatException] on a shape it does not expect; `HttpRemoteApi` turns
/// those into a `ClaxApiError` with code `bad_response`.
library;

typedef Json = Map<String, Object?>;

Json _map(Object? v) => (v as Map).cast<String, Object?>();
int _int(Object? v) => (v as num).toInt();

/// A hosted Clax this device is signed in to, or signing in to.
class Remote {
  const Remote({
    required this.id,
    required this.origin,
    required this.name,
    required this.status,
    this.personName,
    this.personPublicId,
  });

  factory Remote.fromJson(Json j) => Remote(
    id: j['id'] as String,
    origin: j['origin'] as String,
    name: j['name'] as String,
    status: j['status'] as String,
    personName: j['person_name'] as String?,
    personPublicId: j['person_public_id'] as String?,
  );

  final String id;

  /// The normalized origin (`https://clax.dev`).
  final String origin;
  final String name;

  /// `pending` while the sign-in waits, then `connected`. A poll answers
  /// `denied` or `expired` for a sign-in the remote turned down or let
  /// lapse, which the core has then forgotten.
  final String status;

  /// Whose account the sign-in holds, once connected.
  final String? personName;

  /// That account's public ID: what tells it apart from another account
  /// with the same name ([PublishedBy.publicId]).
  final String? personPublicId;

  bool get connected => status == 'connected';

  Json toJson() => {
    'id': id,
    'origin': origin,
    'name': name,
    'status': status,
    'person_name': personName,
    'person_public_id': personPublicId,
  };
}

/// A device sign-in the core started (`POST /api/remotes`): the pending
/// [remote], the [userCode] the approval page shows, that page's
/// [verificationUrl] (the remote's own, with the code), and how many
/// seconds apart to poll ([interval]).
class SignIn {
  const SignIn({
    required this.remote,
    required this.userCode,
    required this.verificationUrl,
    this.interval = 5,
  });

  factory SignIn.fromJson(Json j) => SignIn(
    remote: Remote.fromJson(_map(j['remote'])),
    userCode: j['user_code'] as String,
    verificationUrl: Uri.parse(j['verification_url'] as String),
    interval: switch (j['interval']) {
      final num n when n >= 1 => n.toInt(),
      _ => 5,
    },
  );

  final Remote remote;
  final String userCode;
  final Uri verificationUrl;
  final int interval;
}

/// A local app's link to its public copy on a remote, kept in its lineage
/// entry (`LineageEntry.link`).
class AppLink {
  const AppLink({
    required this.remoteOrigin,
    required this.remoteArtifactId,
    required this.url,
    required this.baseRemoteVersion,
    required this.role,
    this.remoteId,
    this.takenBundleVersion,
    this.mergedSha256,
    this.autoPublish = false,
    this.autoUpdate = false,
    this.pendingBundle,
    this.published = const [],
  });

  /// [role] of a device whose account may publish (owner or editor).
  static const author = 'author';

  /// [role] of a device whose account may only install (viewer or
  /// commenter, or no account).
  static const recipient = 'recipient';

  /// The remote's origin (`https://clax.dev`).
  final String remoteOrigin;

  /// The remote's ID in the core, when this device is signed in to it;
  /// null for an app got by its link alone.
  final String? remoteId;

  /// The public copy's artifact ID on the remote.
  final String remoteArtifactId;

  /// The public copy's address (`<origin>/a/<id>`).
  final String url;

  /// The public artifact version this device last published or took: the
  /// shared base the sync state measures "moved" from.
  final int baseRemoteVersion;

  /// The bundle `version` (the lineage's release counter) this device last
  /// took from the public copy, when it took one.
  final int? takenBundleVersion;

  /// The content hash of the version Bring vN into mine published when this
  /// device last took a release that way: the person's changes combined
  /// with release [takenBundleVersion]. While it is live, a recipient's
  /// modified copy is not a fork (`syncState`). Null after any other take.
  final String? mergedSha256;

  /// Publish each new local version without asking.
  final bool autoPublish;

  /// Take each newer public version without asking (when it needs no new
  /// permission).
  final bool autoUpdate;

  /// [author] or [recipient].
  final String role;

  /// The bundle of the last publish, while it has not reached the public
  /// version it belongs to; its bytes are kept beside the lineages
  /// (`Publisher`).
  final PendingBundle? pendingBundle;

  /// The local versions this device published to the public copy, in the
  /// order it published them: the last was public version
  /// [baseRemoteVersion] when this device last published.
  final List<int> published;

  bool get isAuthor => role == author;

  AppLink copyWith({
    String? Function()? remoteId,
    String? url,
    int? baseRemoteVersion,
    int? Function()? takenBundleVersion,
    String? Function()? mergedSha256,
    bool? autoPublish,
    bool? autoUpdate,
    String? role,
    PendingBundle? Function()? pendingBundle,
    List<int>? published,
  }) => AppLink(
    remoteOrigin: remoteOrigin,
    remoteId: remoteId != null ? remoteId() : this.remoteId,
    remoteArtifactId: remoteArtifactId,
    url: url ?? this.url,
    baseRemoteVersion: baseRemoteVersion ?? this.baseRemoteVersion,
    takenBundleVersion: takenBundleVersion != null
        ? takenBundleVersion()
        : this.takenBundleVersion,
    mergedSha256: mergedSha256 != null ? mergedSha256() : this.mergedSha256,
    autoPublish: autoPublish ?? this.autoPublish,
    autoUpdate: autoUpdate ?? this.autoUpdate,
    role: role ?? this.role,
    pendingBundle: pendingBundle != null
        ? pendingBundle()
        : this.pendingBundle,
    published: published ?? this.published,
  );

  Json toJson() => {
    'remote_origin': remoteOrigin,
    'remote_id': remoteId,
    'remote_artifact_id': remoteArtifactId,
    'url': url,
    'base_remote_version': baseRemoteVersion,
    'taken_bundle_version': takenBundleVersion,
    'merged_sha256': ?mergedSha256,
    'auto_publish': autoPublish,
    'auto_update': autoUpdate,
    'role': role,
    'pending_bundle': ?pendingBundle?.toJson(),
    'published': published,
  };

  /// The link [j] holds, or null when it is not one.
  static AppLink? fromJson(Object? j) {
    if (j is! Map ||
        j['remote_origin'] is! String ||
        j['remote_artifact_id'] is! String ||
        j['url'] is! String ||
        j['base_remote_version'] is! int ||
        (j['role'] != author && j['role'] != recipient)) {
      return null;
    }
    final rid = j['remote_id'];
    final taken = j['taken_bundle_version'];
    final merged = j['merged_sha256'];
    return AppLink(
      remoteOrigin: j['remote_origin'] as String,
      remoteId: rid is String ? rid : null,
      remoteArtifactId: j['remote_artifact_id'] as String,
      url: j['url'] as String,
      baseRemoteVersion: j['base_remote_version'] as int,
      takenBundleVersion: taken is int ? taken : null,
      mergedSha256: merged is String ? merged : null,
      autoPublish: j['auto_publish'] == true,
      autoUpdate: j['auto_update'] == true,
      role: j['role'] as String,
      pendingBundle: PendingBundle.fromJson(j['pending_bundle']),
      published: [
        if (j['published'] case final List<Object?> p)
          for (final n in p) ?(n is int ? n : null),
      ],
    );
  }
}

/// A published bundle on its way to public version [remoteVersion]: of
/// [lineage] at bundle version [bundleVersion], with manifest content hash
/// [contentSha256].
class PendingBundle {
  const PendingBundle({
    required this.remoteVersion,
    required this.lineage,
    required this.bundleVersion,
    required this.contentSha256,
  });

  final int remoteVersion;
  final String lineage;
  final int bundleVersion;
  final String contentSha256;

  Json toJson() => {
    'remote_version': remoteVersion,
    'lineage': lineage,
    'bundle_version': bundleVersion,
    'content_sha256': contentSha256,
  };

  /// The pending bundle [j] holds, or null when it is not one.
  static PendingBundle? fromJson(Object? j) {
    if (j is! Map ||
        j['remote_version'] is! int ||
        j['lineage'] is! String ||
        j['bundle_version'] is! int ||
        j['content_sha256'] is! String) {
      return null;
    }
    return PendingBundle(
      remoteVersion: j['remote_version'] as int,
      lineage: j['lineage'] as String,
      bundleVersion: j['bundle_version'] as int,
      contentSha256: j['content_sha256'] as String,
    );
  }
}

/// The account that published a public version, or owns the public copy.
class PublishedBy {
  const PublishedBy({required this.name, this.login, this.publicId});

  factory PublishedBy.fromJson(Json j) => PublishedBy(
    name: j['name'] as String,
    login: j['login'] as String?,
    publicId: j['public_id'] as String?,
  );

  final String name;

  /// Its GitHub login, when it has one.
  final String? login;

  /// The account's public ID on its hosted Clax; null for the hosted
  /// Clax's owner, or a head from before it was reported. Names and
  /// logins are the account's to choose: only this tells two accounts
  /// apart.
  final String? publicId;

  Json toJson() => {'name': name, 'login': login, 'public_id': publicId};

  /// How the app names the account: `@login`, else its name.
  String get handle => login == null ? name : '@$login';
}

/// Who made a version that came from elsewhere, for the person's trust in
/// its author before Claude reads it: a bundle's signer, or the hosted
/// account that published a version without one.
class VersionSource {
  const VersionSource({
    required this.key,
    required this.who,
    this.origin,
    this.account,
  });

  /// The version [by] published on the hosted Clax at [origin], keyed by
  /// the account's public ID; an account without one is keyed apart from
  /// every account that has one, so a name never stands for an ID.
  factory VersionSource.account(String origin, PublishedBy by) =>
      VersionSource(
        key: switch (by.publicId) {
          final id? => 'account:$origin/$id',
          null => 'account:$origin/no-id:${by.name}',
        },
        who: by.handle,
        origin: origin,
        account: by,
      );

  /// What the person's trust is kept under: the signer key (base64url), or
  /// `account:<origin>/<public ID>`.
  final String key;

  /// How the app names the author.
  final String who;

  /// The hosted Clax the version came from, when it came from one.
  final String? origin;

  /// The account that published it there, when it came from one.
  final PublishedBy? account;

  Json toJson() => {
    'key': key,
    'who': who,
    'origin': origin,
    'account': account?.toJson(),
  };

  /// [j] as written by [toJson], or null for any other shape.
  static VersionSource? fromJson(Object? j) {
    if (j case {'key': final String key, 'who': final String who}) {
      return VersionSource(
        key: key,
        who: who,
        origin: j['origin'] is String ? j['origin'] as String : null,
        account: switch (j['account']) {
          {'name': String _} && final Map a => PublishedBy.fromJson(
            a.cast<String, Object?>(),
          ),
          _ => null,
        },
      );
    }
    return null;
  }
}

/// `GET /api/artifacts/<id>/head` on a remote: the public copy's current
/// version, and the latest version that carries a bundle.
class RemoteHead {
  const RemoteHead({
    required this.artifactId,
    required this.url,
    required this.title,
    required this.version,
    required this.contentSha256,
    required this.publishedBy,
    required this.owner,
    required this.comments,
    this.note,
    this.capabilities,
    this.bundle,
  });

  factory RemoteHead.fromJson(Json j) => RemoteHead(
    artifactId: j['artifact_id'] as String,
    url: j['url'] as String,
    title: j['title'] as String,
    version: _int(j['version']),
    contentSha256: j['content_sha256'] as String,
    publishedBy: PublishedBy.fromJson(_map(j['published_by'])),
    owner: PublishedBy.fromJson(_map(j['owner'])),
    comments: j['comments'] as bool,
    note: switch (j['note']) {
      final String n when n.trim().isNotEmpty => n,
      _ => null,
    },
    capabilities: switch (j['capabilities']) {
      final Map<String, Object?> c => c,
      _ => null,
    },
    bundle: j['bundle'] == null
        ? null
        : RemoteBundle.fromJson(_map(j['bundle'])),
  );

  final String artifactId;
  final String url;
  final String title;

  /// The current public artifact version.
  final int version;

  /// The current version's content hash, `sha256:<hex>`.
  final String contentSha256;
  final PublishedBy publishedBy;
  final PublishedBy owner;

  /// Whether the public copy takes comments.
  final bool comments;

  /// The current version's change note; null when it has none.
  final String? note;

  /// The public copy's capability declaration; null from a remote that
  /// does not report it.
  final Map<String, Object?>? capabilities;

  /// The latest version that carries a bundle; null when none does.
  final RemoteBundle? bundle;

  /// True when the current version was made on the web, without a bundle:
  /// [bundle] is an earlier version's, or there is none.
  bool get webMade => bundle?.version != version;
}

/// A bundle a public version carries.
class RemoteBundle {
  const RemoteBundle({
    required this.version,
    required this.lineage,
    required this.bundleVersion,
    required this.contentSha256,
    required this.url,
    required this.publishedBy,
  });

  factory RemoteBundle.fromJson(Json j) => RemoteBundle(
    version: _int(j['version']),
    lineage: j['lineage'] as String,
    bundleVersion: _int(j['bundle_version']),
    contentSha256: j['content_sha256'] as String,
    url: j['url'] as String,
    publishedBy: PublishedBy.fromJson(_map(j['published_by'])),
  );

  /// The public artifact version that carries it.
  final int version;
  final String lineage;

  /// The bundle's own `version`, the lineage's release counter.
  final int bundleVersion;

  /// The bundle's manifest content hash, `sha256:<hex>`.
  final String contentSha256;

  /// Where its bytes are (the remote's GET bundle route).
  final String url;
  final PublishedBy publishedBy;
}

/// An app in the signed-in account's library on a remote
/// (`GET /api/hosted/apps`).
class RemoteApp {
  const RemoteApp({
    required this.artifactId,
    required this.title,
    required this.url,
    required this.version,
    required this.bundleVersion,
    required this.access,
    required this.general,
  });

  factory RemoteApp.fromJson(Json j) => RemoteApp(
    artifactId: j['artifact_id'] as String,
    title: j['title'] as String,
    url: j['url'] as String,
    version: _int(j['version']),
    bundleVersion: _int(j['bundle_version']),
    access: j['access'] as String,
    general: j['general'] as String,
  );

  final String artifactId;
  final String title;
  final String url;
  final int version;
  final int bundleVersion;

  /// `owner`, `editor`, `commenter` or `viewer`.
  final String access;

  /// `restricted`, `link` or `public`.
  final String general;

  /// Whether this account may publish it (owner or editor).
  bool get mayPublish => access == 'owner' || access == 'editor';
}

/// One person a public copy is shared with.
class RemoteShare {
  const RemoteShare({
    required this.id,
    required this.access,
    required this.pending,
    this.githubLogin,
    this.person,
    this.name,
  });

  factory RemoteShare.fromJson(Json j) => RemoteShare(
    id: j['id'] as String,
    access: j['access'] as String,
    githubLogin: j['github_login'] as String?,
    person: j['person'] as String?,
    name: j['name'] as String?,
    pending: j['pending'] as bool,
  );

  final String id;

  /// `viewer`, `commenter` or `editor`.
  final String access;
  final String? githubLogin;

  /// The person's public ID, once known.
  final String? person;
  final String? name;

  /// True until the GitHub login first signs in.
  final bool pending;
}

/// A person the sharing dialog suggests.
class ShareCandidate {
  const ShareCandidate({required this.person, this.name, this.githubLogin});

  factory ShareCandidate.fromJson(Json j) => ShareCandidate(
    person: j['person'] as String,
    name: j['name'] as String?,
    githubLogin: j['github_login'] as String?,
  );

  final String person;
  final String? name;
  final String? githubLogin;
}

/// A public copy's sharing, as `GET /api/hosted/artifacts/<id>/sharing`
/// answers it: everything for whoever may share it, else [canShare] false
/// and nothing else.
class Sharing {
  const Sharing({
    required this.artifactId,
    required this.canShare,
    this.general,
    this.publicGallery = false,
    this.linkAccess,
    this.url,
    this.people = const [],
    this.suggestions = const [],
    this.comments = true,
  });

  factory Sharing.fromJson(Json j) {
    final can = j['can_share'] as bool;
    if (!can) {
      return Sharing(artifactId: j['artifact_id'] as String, canShare: false);
    }
    return Sharing(
      artifactId: j['artifact_id'] as String,
      canShare: true,
      general: j['general'] as String,
      publicGallery: j['public_gallery'] == true,
      linkAccess: j['link_access'] as String?,
      url: j['url'] as String,
      people: [
        for (final p in j['people'] as List<Object?>)
          RemoteShare.fromJson(_map(p)),
      ],
      suggestions: [
        for (final p in (j['suggestions'] as List<Object?>?) ?? const [])
          ShareCandidate.fromJson(_map(p)),
      ],
      comments: j['comments'] as bool? ?? true,
    );
  }

  final String artifactId;
  final bool canShare;

  /// `restricted`, `link` or `public`.
  final String? general;
  final bool publicGallery;

  /// `viewer` or `commenter` when shared by link.
  final String? linkAccess;
  final String? url;
  final List<RemoteShare> people;
  final List<ShareCandidate> suggestions;

  /// Whether the public copy takes comments.
  final bool comments;
}

/// One local version a publish sent, and the public version it made.
class PushedVersion {
  const PushedVersion({required this.n, required this.remoteN});

  final int n;
  final int remoteN;
}

/// The answer of a publish (`POST /api/artifacts/<aid>/push`, app mode).
class PushResult {
  const PushResult({
    required this.url,
    required this.remote,
    required this.remoteArtifactId,
    required this.remoteVersion,
    required this.pushed,
  });

  factory PushResult.fromJson(Json j) {
    final link = _map(j['link']);
    return PushResult(
      url: j['url'] as String,
      remote: Remote.fromJson(_map(j['remote'])),
      remoteArtifactId: link['remote_artifact_id'] as String,
      remoteVersion: _int(link['remote_n']),
      pushed: [
        for (final p in j['pushed'] as List<Object?>)
          PushedVersion(
            n: _int(_map(p)['n']),
            remoteN: _int(_map(p)['remote_n']),
          ),
      ],
    );
  }

  /// The public copy's address.
  final String url;
  final Remote remote;
  final String remoteArtifactId;

  /// The public version the link now names.
  final int remoteVersion;
  final List<PushedVersion> pushed;
}

/// Where a bundle being opened came from: public artifact
/// [remoteArtifactId] on [remoteOrigin], at public version
/// [remoteVersion], with the [head] it was fetched by.
class BundleOrigin {
  const BundleOrigin({
    required this.remoteOrigin,
    required this.remoteArtifactId,
    required this.remoteVersion,
    required this.head,
  });

  final String remoteOrigin;
  final String remoteArtifactId;
  final int remoteVersion;
  final RemoteHead head;

  /// Whether [link] names this bundle's public copy.
  bool names(AppLink? link) =>
      link != null &&
      link.remoteOrigin == remoteOrigin &&
      link.remoteArtifactId == remoteArtifactId;
}
