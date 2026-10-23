/// An in-memory [RemoteApi] for tests and previews.
library;

import 'dart:typed_data';

import '../bundle/bundle_reader.dart' show maxBundleBytes;
import '../clax/api.dart' show ClaxApiError;
import 'models.dart';
import 'remote_api.dart';

/// One public version of a [FakePublicCopy].
class FakePublicVersion {
  FakePublicVersion({
    required this.n,
    required this.contentSha256,
    required this.publishedBy,
    this.files = const {},
    this.note,
  });

  final int n;

  /// Its change note, when it has one.
  final String? note;
  final String contentSha256;
  final PublishedBy publishedBy;
  final Map<String, Uint8List> files;

  /// The bundle this version carries, once one was attached.
  FakeBundle? bundle;
}

/// A bundle attached to a [FakePublicVersion].
class FakeBundle {
  const FakeBundle({
    required this.bytes,
    required this.lineage,
    required this.bundleVersion,
    required this.contentSha256,
  });

  final Uint8List bytes;
  final String lineage;
  final int bundleVersion;
  final String contentSha256;
}

/// A public copy on a fake remote.
class FakePublicCopy {
  FakePublicCopy({
    required this.rid,
    required this.raid,
    required this.url,
    required this.title,
    required this.owner,
    this.comments = false,
    this.general = 'restricted',
    this.access = 'owner',
  });

  /// The remote it was made through; it lives at that remote's origin,
  /// which a later sign-in there reaches too ([origin]).
  final String rid;
  final String raid;

  /// The hosted Clax it is on.
  String get origin => Uri.parse(url).origin;
  final String url;
  String title;
  final PublishedBy owner;
  bool comments;

  /// `restricted`, `link` or `public`.
  String general;

  /// What the signed-in account holds on it: `owner`, `editor`,
  /// `commenter` or `viewer`.
  String access;
  String? linkAccess;
  bool listed = false;

  /// The public artifact this copy was made from, as its first push named
  /// it (a fork's `forked_from`).
  String? forkedFrom;

  /// The copy's capability declaration, as its head reports it; null for
  /// a head that reports none.
  Map<String, Object?>? capabilities;
  final versions = <FakePublicVersion>[];
  final shares = <RemoteShare>[];

  /// The comments posted on it, in order.
  final posted = <String>[];
  final installs = <String>{};

  FakePublicVersion get current => versions.last;

  /// The latest version that carries a bundle, or null.
  FakePublicVersion? get bundled {
    for (final v in versions.reversed) {
      if (v.bundle != null) return v;
    }
    return null;
  }

  String bundleUrl(int n) {
    final u = Uri.parse(url);
    return u.replace(path: '/api/artifacts/$raid/versions/$n/bundle').toString();
  }

  RemoteHead get head {
    final b = bundled;
    return RemoteHead(
      artifactId: raid,
      url: url,
      title: title,
      version: current.n,
      contentSha256: current.contentSha256,
      publishedBy: current.publishedBy,
      owner: owner,
      comments: comments,
      note: current.note,
      capabilities: capabilities,
      bundle: b == null
          ? null
          : RemoteBundle(
              version: b.n,
              lineage: b.bundle!.lineage,
              bundleVersion: b.bundle!.bundleVersion,
              contentSha256: b.bundle!.contentSha256,
              url: bundleUrl(b.n),
              publishedBy: b.publishedBy,
            ),
    );
  }
}

/// A [RemoteApi] whose remotes and public copies live in memory. It
/// refuses what the core and the remote refuse (404 for a remote or copy
/// it does not hold, 409 `remote_moved`, 413 `bundle_too_large`; a bundle
/// from an account that may not publish the copy, whose content hash is
/// not its version's (`bundle_mismatch`), of another lineage than the
/// copy's earlier bundles, or whose bundle version does not go up), and
/// throws [StateError] on a call the
/// test did not program: a publish of a local version not given to
/// [localVersion], or a poll of a sign-in neither [approve]d, [deny]d
/// nor [expire]d unless [signInsWait].
class FakeRemoteApi implements RemoteApi {
  /// Every remote, by ID.
  final remotesById = <String, Remote>{};

  /// Every public copy, by artifact ID.
  final copies = <String, FakePublicCopy>{};

  /// The local apps' links: the public copy each publishes to, and the
  /// public version it last pushed or linked at.
  final links = <String, ({String rid, String raid, int remoteN})>{};

  /// The calls made, by method name.
  final calls = <String>[];

  /// The local apps whose files use assets.
  final usesAssets = <String>{};

  /// Who publishes from this device.
  PublishedBy me = const PublishedBy(name: 'Me', login: 'me');

  final _local = <String, ({String contentSha256, String title})>{};
  final _failures = <String, List<Object>>{};

  /// Makes the next call of [method] (a [RemoteApi] method's name) throw
  /// [error] instead of running; queued in order when given more than once.
  void failNext(String method, Object error) =>
      (_failures[method] ??= []).add(error);

  /// Records a call of [method], throwing a failure [failNext] queued.
  void _call(String method) {
    calls.add(method);
    final queued = _failures[method];
    if (queued != null && queued.isNotEmpty) throw queued.removeAt(0);
  }

  /// The origins whose hosted Clax is older than this app: every call
  /// that reaches one is answered as the core answers it, 502
  /// `remote_too_old` naming the origin. Calls the core answers by itself
  /// (the remotes, linking and unlinking) still work.
  final tooOld = <String>{};

  /// Throws the core's `remote_too_old` when [origin] is in [tooOld].
  void _reach(String origin) {
    final o = Uri.parse(origin.contains('://') ? origin : 'https://$origin')
        .origin;
    if (tooOld.contains(o)) {
      throw ClaxApiError(
        status: 502,
        code: 'remote_too_old',
        message: '$o is too old for this request',
      );
    }
  }

  /// [_reach] for remote [rid]'s origin.
  void _reachRemote(String rid) => _reach(_remote(rid).origin);
  final _answers = <String, String>{};
  var _ids = 0;

  /// Programs local version [n] of [aid]: what a publish of it sends.
  void localVersion(
    String aid,
    int n, {
    required String contentSha256,
    String title = 'App',
  }) => _local['$aid/$n'] = (contentSha256: contentSha256, title: title);

  /// Adds a remote signed in already.
  Remote connected(String origin, {String? name, String person = 'Me'}) {
    final r = Remote(
      id: 'r${++_ids}',
      origin: origin,
      name: name ?? Uri.parse(origin).host,
      status: 'connected',
      personName: person,
    );
    remotesById[r.id] = r;
    return r;
  }

  /// Approves [rid]'s pending sign-in: its next poll connects.
  void approve(String rid) => _answers[rid] = 'connected';

  /// Denies [rid]'s pending sign-in: its next poll forgets it.
  void deny(String rid) => _answers[rid] = 'denied';

  /// Lets [rid]'s pending sign-in lapse on the remote: its next poll
  /// answers `expired` and forgets it.
  void expire(String rid) => _answers[rid] = 'expired';

  /// Whether a poll of a sign-in neither [approve]d, [deny]d nor
  /// [expire]d answers `pending` (the person has not acted yet) rather
  /// than throwing.
  bool signInsWait = false;

  /// The longest client name the remote takes, in characters, once
  /// trimmed.
  static const maxMachineChars = 64;

  /// Whether [c] is a control character (Unicode Cc), which a client name
  /// may not hold.
  static bool isControlRune(int c) => c < 0x20 || (c >= 0x7f && c <= 0x9f);

  /// Each sign-in [addRemote] started, in order.
  final signIns = <({String url, String kind, String? machine})>[];

  /// Adds a public copy on [rid] someone else made, at version 1.
  FakePublicCopy addCopy(
    String rid, {
    required String contentSha256,
    String title = 'App',
    PublishedBy owner = const PublishedBy(name: 'Owner', login: 'owner'),
    String access = 'viewer',
    String general = 'link',
    bool comments = true,
  }) {
    final r = _remote(rid);
    final raid = 'ra${++_ids}';
    final c = FakePublicCopy(
      rid: rid,
      raid: raid,
      url: '${r.origin}/a/$raid',
      title: title,
      owner: owner,
      access: access,
      general: general,
      comments: comments,
    );
    c.versions.add(
      FakePublicVersion(n: 1, contentSha256: contentSha256, publishedBy: owner),
    );
    copies[raid] = c;
    return c;
  }

  /// Publishes a new version of [raid] as someone else would: with a
  /// bundle ([bundle] non-null) or from the web, holding [files] (what
  /// [versionFiles] answers for it).
  FakePublicVersion publishThere(
    String raid, {
    required String contentSha256,
    FakeBundle? bundle,
    PublishedBy? by,
    Map<String, Uint8List> files = const {},
    String? note,
  }) {
    final c = _copy(null, raid);
    final v = FakePublicVersion(
      n: c.current.n + 1,
      contentSha256: contentSha256,
      publishedBy: by ?? c.owner,
      files: files,
      note: note,
    )..bundle = bundle;
    c.versions.add(v);
    return v;
  }

  Remote _remote(String rid) =>
      remotesById[rid] ??
      (throw const ClaxApiError(
        status: 404,
        code: 'not_found',
        message: 'no such remote',
      ));

  /// [rid]'s origin, as its copies are matched by.
  String _originOf(String rid) => Uri.parse(_remote(rid).origin).origin;

  FakePublicCopy _copy(String? rid, String raid) {
    final c = copies[raid];
    if (c == null || (rid != null && c.origin != _originOf(rid))) {
      throw const ClaxApiError(
        status: 404,
        code: 'not_found',
        message: 'no such artifact',
      );
    }
    return c;
  }

  FakePublicCopy _byUrl(Uri artifactUrl) {
    for (final c in copies.values) {
      if (Uri.parse(c.url) == artifactUrl && c.general != 'restricted') {
        return c;
      }
    }
    throw const ClaxApiError(
      status: 404,
      code: 'not_found',
      message: 'no such artifact',
    );
  }

  void _mayShare(FakePublicCopy c) {
    if (c.access != 'owner') {
      throw const ClaxApiError(
        status: 403,
        code: 'not_permitted',
        message: 'only the owner shares it',
      );
    }
  }

  @override
  Future<List<Remote>> remotes() async {
    _call('remotes');
    return remotesById.values.toList();
  }

  @override
  Future<SignIn> addRemote(
    String url, {
    String kind = 'clax-app',
    String? machine,
  }) async {
    _call('addRemote');
    signIns.add((url: url, kind: kind, machine: machine));
    final origin = url.contains('://') ? url : 'https://$url';
    _reach(origin);
    for (final r in remotesById.values) {
      if (r.origin == origin && r.connected) {
        throw const ClaxApiError(
          status: 400,
          code: 'remote_exists',
          message: 'signed in there already',
        );
      }
    }
    // The remote's own rule for a client name (clax-store's
    // checked_client_name), which the core passes on as 502.
    if (machine != null &&
        (machine.trim().runes.length > maxMachineChars ||
            machine.trim().runes.any(isControlRune))) {
      throw const ClaxApiError(
        status: 502,
        code: 'remote_refused',
        message: 'the client name is too long or has a control character',
      );
    }
    // The core keeps one pending sign-in per origin: a new one replaces it.
    remotesById.removeWhere((_, r) => r.origin == origin && !r.connected);

    final r = Remote(
      id: 'r${++_ids}',
      origin: origin,
      name: Uri.parse(origin).host,
      status: 'pending',
    );
    remotesById[r.id] = r;
    final code = 'CODE-${r.id.toUpperCase()}';
    return SignIn(
      remote: r,
      userCode: code,
      verificationUrl: Uri.parse('$origin/device?code=$code'),
    );
  }

  @override
  Future<Remote> poll(String rid) async {
    _call('poll');
    final r = _remote(rid);
    _reach(r.origin);
    if (r.connected) {
      throw const ClaxApiError(
        status: 400,
        code: 'not_pending',
        message: 'connected already',
      );
    }
    Remote as(String status) => Remote(
      id: r.id,
      origin: r.origin,
      name: r.name,
      status: status,
      personName: status == 'connected' ? me.name : null,
    );
    final answer = _answers.remove(rid);
    if (answer == 'connected') return remotesById[rid] = as('connected');
    if (answer != null) {
      remotesById.remove(rid);
      return as(answer);
    }
    if (signInsWait) return r;
    throw StateError('poll of $rid: neither approved nor denied');
  }

  @override
  Future<bool> removeRemote(String rid) async {
    _call('removeRemote');
    final r = _remote(rid);
    remotesById.remove(rid);
    links.removeWhere((_, l) => l.rid == rid);
    // The core forgets the remote here whatever the revoke came to.
    try {
      _reach(r.origin);
    } on ClaxApiError {
      return false;
    }
    return revokes;
  }

  /// Whether a remote revokes this device's token when it disconnects;
  /// false models a remote out of reach or refusing, which the core
  /// answers `revoked: false` for.
  bool revokes = true;

  @override
  Future<PushResult> publish(
    String aid, {
    required int version,
    String? rid,
    String? title,
    String? forkedFrom,
  }) async {
    _call('publish');
    final Remote r;
    if (rid != null) {
      r = _remote(rid);
    } else {
      final on = remotesById.values.where((r) => r.connected).toList();
      if (on.length != 1) {
        throw ClaxApiError(
          status: on.isEmpty ? 404 : 400,
          code: on.isEmpty ? 'no_remote' : 'remote_required',
          message: 'name a remote',
        );
      }
      r = on.single;
    }
    _reach(r.origin);
    if (!r.connected) {
      throw const ClaxApiError(
        status: 400,
        code: 'not_connected',
        message: 'the sign-in is not finished',
      );
    }
    final local =
        _local['$aid/$version'] ??
        (throw StateError('publish of $aid v$version: not programmed'));
    if (usesAssets.contains(aid)) {
      throw const UsesAssets('the app uses assets');
    }
    final link = links[aid];
    final FakePublicCopy c;
    if (link == null || link.rid != r.id) {
      final raid = 'ra${++_ids}';
      c = FakePublicCopy(
        rid: r.id,
        raid: raid,
        url: '${r.origin}/a/$raid',
        title: title ?? local.title,
        owner: me,
      )..forkedFrom = forkedFrom;
      copies[raid] = c;
    } else {
      c = _copy(r.id, link.raid);
      if (!const {'owner', 'editor'}.contains(c.access)) {
        throw const ClaxApiError(
          status: 403,
          code: 'not_permitted',
          message: 'this account may not publish it',
        );
      }
      if (c.current.n != link.remoteN) throw RemoteMoved(c.current.n);
    }
    final n = c.versions.isEmpty ? 1 : c.current.n + 1;
    c.versions.add(
      FakePublicVersion(
        n: n,
        contentSha256: local.contentSha256,
        publishedBy: me,
      ),
    );
    if (link != null && link.rid == r.id) c.title = local.title;
    links[aid] = (rid: r.id, raid: c.raid, remoteN: n);
    return PushResult(
      url: c.url,
      remote: r,
      remoteArtifactId: c.raid,
      remoteVersion: n,
      pushed: [PushedVersion(n: version, remoteN: n)],
    );
  }

  @override
  Future<void> putBundle(
    String aid,
    String rid,
    int remoteVersion,
    Uint8List bytes, {
    required String lineage,
    required int bundleVersion,
    required String contentSha256,
  }) async {
    _call('putBundle');
    _reachRemote(rid);
    final link =
        links[aid] ??
        (throw const ClaxApiError(
          status: 400,
          code: 'no_link',
          message: 'not published',
        ));
    final c = _copy(rid, link.raid);
    if (!const {'owner', 'editor'}.contains(c.access)) {
      throw const ClaxApiError(
        status: 403,
        code: 'not_permitted',
        message: 'this account may not publish it',
      );
    }
    if (bytes.length > maxBundleBytes) {
      throw const ClaxApiError(
        status: 413,
        code: 'bundle_too_large',
        message: 'over 32 MiB',
      );
    }
    final v = c.versions.where((v) => v.n == remoteVersion).firstOrNull;
    if (v == null) {
      throw const ClaxApiError(
        status: 404,
        code: 'not_found',
        message: 'no such version',
      );
    }
    final had = v.bundle;
    if (had != null) {
      if (_same(had.bytes, bytes)) return;
      throw const ClaxApiError(
        status: 400,
        code: 'bundle_exists',
        message: 'this version carries a bundle already',
      );
    }
    if (contentSha256 != v.contentSha256) {
      throw const ClaxApiError(
        status: 400,
        code: 'bundle_mismatch',
        message: 'the bundle is not this version\'s files',
      );
    }
    final last = c.bundled?.bundle;
    if (last != null && lineage != last.lineage) {
      throw const ClaxApiError(
        status: 400,
        code: 'bundle_lineage',
        message: 'the copy\'s bundles are of another lineage',
      );
    }
    if (last != null && bundleVersion <= last.bundleVersion) {
      throw const ClaxApiError(
        status: 400,
        code: 'bundle_version',
        message: 'the bundle version must go up',
      );
    }
    v.bundle = FakeBundle(
      bytes: bytes,
      lineage: lineage,
      bundleVersion: bundleVersion,
      contentSha256: contentSha256,
    );
  }

  static bool _same(Uint8List a, Uint8List b) {
    if (a.length != b.length) return false;
    for (var i = 0; i < a.length; i++) {
      if (a[i] != b[i]) return false;
    }
    return true;
  }

  @override
  Future<void> link(
    String rid,
    String aid,
    RemoteApp app,
    int remoteVersion,
  ) async {
    _call('link');
    _remote(rid);
    _copy(rid, app.artifactId);
    links[aid] = (rid: rid, raid: app.artifactId, remoteN: remoteVersion);
  }

  @override
  Future<void> unlink(String aid, String rid) async {
    _call('unlink');
    _remote(rid);
    final l = links[aid];
    if (l == null || l.rid != rid) {
      throw const ClaxApiError(
        status: 404,
        code: 'not_found',
        message: 'no such link',
      );
    }
    links.remove(aid);
  }

  @override
  Future<void> comment(String rid, String raid, String body) async {
    _call('comment');
    _reachRemote(rid);
    final c = _copy(rid, raid);
    if (!c.comments) {
      throw const ClaxApiError(
        status: 403,
        code: 'comments_off',
        message: 'comments are off',
      );
    }
    if (c.access == 'viewer') {
      throw const ClaxApiError(
        status: 403,
        code: 'not_permitted',
        message: 'this account may only view it',
      );
    }
    if (body.trim().isEmpty) {
      throw const ClaxApiError(
        status: 400,
        code: 'invalid_args',
        message: 'a comment needs text',
      );
    }
    c.posted.add(body);
  }

  @override
  Future<RemoteHead> head(String rid, String raid) async {
    _call('head');
    _reachRemote(rid);
    return _copy(rid, raid).head;
  }

  @override
  Future<RemoteHead> anonymousHead(Uri artifactUrl) async {
    _call('anonymousHead');
    _reach(artifactUrl.origin);
    return _byUrl(artifactUrl).head;
  }

  @override
  Future<Uint8List> bundle(String rid, String raid, int n) async {
    _call('bundle');
    _reachRemote(rid);
    final c = _copy(rid, raid);
    return c.versions.where((v) => v.n == n).firstOrNull?.bundle?.bytes ??
        (throw const ClaxApiError(
          status: 404,
          code: 'not_found',
          message: 'no bundle',
        ));
  }

  @override
  Future<Uint8List> anonymousBundle(Uri url) async {
    _call('anonymousBundle');
    _reach(url.origin);
    for (final c in copies.values) {
      if (c.general == 'restricted') continue;
      for (final v in c.versions) {
        if (v.bundle != null && Uri.parse(c.bundleUrl(v.n)) == url) {
          return v.bundle!.bytes;
        }
      }
    }
    throw const ClaxApiError(
      status: 404,
      code: 'not_found',
      message: 'no bundle',
    );
  }

  @override
  Future<Map<String, Uint8List>> versionFiles(
    String rid,
    String raid,
    int n,
  ) async {
    _call('versionFiles');
    _reachRemote(rid);
    final v = _copy(rid, raid).versions.where((v) => v.n == n).firstOrNull;
    if (v == null) {
      throw const ClaxApiError(
        status: 404,
        code: 'not_found',
        message: 'no such version',
      );
    }
    return {...v.files};
  }

  @override
  Future<List<RemoteApp>> apps(String rid) async {
    _call('apps');
    _reachRemote(rid);
    return [
      for (final c in copies.values)
        if (c.origin == _originOf(rid) && c.bundled != null)
          RemoteApp(
            artifactId: c.raid,
            title: c.title,
            url: c.url,
            version: c.current.n,
            bundleVersion: c.bundled!.bundle!.bundleVersion,
            access: c.access,
            general: c.general,
          ),
    ];
  }

  @override
  Future<Sharing> sharing(String rid, String raid) async {
    _call('sharing');
    _reachRemote(rid);
    final c = _copy(rid, raid);
    if (c.access != 'owner') {
      return Sharing(artifactId: raid, canShare: false);
    }
    return Sharing(
      artifactId: raid,
      canShare: true,
      general: c.general,
      publicGallery: c.listed,
      linkAccess: c.linkAccess,
      url: c.url,
      people: [...c.shares],
      comments: c.comments,
    );
  }

  /// The names of people the remote knows, by public ID: a share by
  /// person takes its name from here.
  final personNames = <String, String>{};

  @override
  Future<void> share(
    String rid,
    String raid,
    ShareTarget to,
    String access,
  ) async {
    _call('share');
    _reachRemote(rid);
    final c = _copy(rid, raid);
    _mayShare(c);
    if (!const {'viewer', 'commenter', 'editor'}.contains(access)) {
      throw const ClaxApiError(
        status: 400,
        code: 'invalid_args',
        message: 'no such access',
      );
    }
    switch (to) {
      case ShareLogin(:final login):
        c.shares
          ..removeWhere((s) => s.githubLogin == login)
          ..add(
            RemoteShare(
              id: 's${++_ids}',
              access: access,
              githubLogin: login,
              pending: true,
            ),
          );
      case SharePerson(:final person):
        final name = personNames[person];
        if (name == null) {
          throw const ClaxApiError(
            status: 400,
            code: 'unknown_person',
            message: 'no such person',
          );
        }
        c.shares
          ..removeWhere((s) => s.person == person)
          ..add(
            RemoteShare(
              id: 's${++_ids}',
              access: access,
              person: person,
              name: name,
              pending: false,
            ),
          );
    }
  }

  @override
  Future<void> unshare(String rid, String raid, String sid) async {
    _call('unshare');
    _reachRemote(rid);
    final c = _copy(rid, raid);
    _mayShare(c);
    final before = c.shares.length;
    c.shares.removeWhere((s) => s.id == sid);
    if (c.shares.length == before) {
      throw const ClaxApiError(
        status: 404,
        code: 'not_found',
        message: 'no such share',
      );
    }
  }

  @override
  Future<void> setLink(
    String rid,
    String raid,
    String? access, {
    bool listed = false,
  }) async {
    _call('setLink');
    _reachRemote(rid);
    final c = _copy(rid, raid);
    _mayShare(c);
    if (access != null && access != 'viewer' && access != 'commenter') {
      throw const ClaxApiError(
        status: 400,
        code: 'invalid_args',
        message: 'no such access',
      );
    }
    c.linkAccess = access;
    c.listed = access != null && listed;
    c.general = access == null
        ? 'restricted'
        : listed
        ? 'public'
        : 'link';
  }

  @override
  Future<void> setComments(String rid, String raid, bool on) async {
    _call('setComments');
    _reachRemote(rid);
    final c = _copy(rid, raid);
    _mayShare(c);
    c.comments = on;
  }

  @override
  Future<void> unpublish(String rid, String raid) async {
    _call('unpublish');
    _reachRemote(rid);
    final c = _copy(rid, raid);
    _mayShare(c);
    copies.remove(raid);
    links.removeWhere((_, l) => l.raid == raid);
  }

  /// As the core's route forwards it: counted whatever the copy's general
  /// access, for an account that may open it; the remote's 400 for an
  /// install that is not 64 hex comes back as 502 `remote_refused`.
  @override
  Future<void> countInstall(String rid, String raid, String installId) async {
    _call('countInstall');
    _reachRemote(rid);
    final c = _copy(rid, raid);
    if (!RegExp(r'^[0-9a-f]{64}$').hasMatch(installId)) {
      throw const ClaxApiError(
        status: 502,
        code: 'remote_refused',
        message: 'install is 64 hex',
        body: {'remote_status': 400, 'remote_code': 'invalid_args'},
      );
    }
    c.installs.add(installId);
  }

  /// Without a token, as the remote's route takes it: a copy not shared
  /// by link is 404 there.
  @override
  Future<void> recordInstall(Uri artifactUrl, String installId) async {
    _call('recordInstall');
    _reach(artifactUrl.origin);
    if (!RegExp(r'^[0-9a-f]{64}$').hasMatch(installId)) {
      throw const ClaxApiError(
        status: 400,
        code: 'invalid_args',
        message: 'install is 64 hex',
      );
    }
    _byUrl(artifactUrl).installs.add(installId);
  }
}
