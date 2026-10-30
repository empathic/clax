/// The app's client of the hosted Clax instances it publishes apps to and
/// gets them from: the core's remote routes, which carry this device's
/// token there, and the remote's public routes, for apps got by their
/// link alone.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import '../bundle/bundle_reader.dart' show maxBundleBytes;
import '../bundle/exporter.dart' show bundleMime;
import '../clax/api.dart' show ClaxApiError;
import '../clax/transport.dart';
import 'models.dart';

/// A publish refused because the public copy has versions this device
/// did not publish: it is at [remoteVersion] now (409 `remote_moved`).
class RemoteMoved implements Exception {
  const RemoteMoved(this.remoteVersion);

  final int remoteVersion;

  @override
  String toString() => 'RemoteMoved($remoteVersion)';
}

/// A publish refused because the app's files use assets, which an app's
/// public copy cannot carry (400 `uses_assets`).
class UsesAssets implements Exception {
  const UsesAssets(this.message);

  final String message;

  @override
  String toString() => 'UsesAssets: $message';
}

/// The remotes and the public copies on them. Every refusal other than
/// [RemoteMoved] and [UsesAssets] throws `ClaxApiError`.
abstract interface class RemoteApi {
  /// Every remote this device is signed in to or signing in to.
  Future<List<Remote>> remotes();

  /// Starts a device sign-in at [url], as a [kind] client named [machine]:
  /// the pending remote, with the code and page the person approves it on.
  Future<SignIn> addRemote(
    String url, {
    String kind = 'clax-app',
    String? machine,
  });

  /// Asks whether [rid]'s sign-in was approved: the remote with the poll's
  /// status as its [Remote.status] (`pending`, `connected`, or `denied` or
  /// `expired` for one the core has forgotten).
  Future<Remote> poll(String rid);

  /// Signs out of [rid] and forgets it, whether or not the remote revoked
  /// this device's token: answers whether it did (false when the remote
  /// could not be reached or refused, so the token stays valid there until
  /// it is removed on the remote).
  Future<bool> removeRemote(String rid);

  /// Publishes local version [version] of [aid] as the next public version
  /// on [rid] (the only connected remote when null). A push that makes a
  /// new public copy names it [title] (the app's own when null) and, for a
  /// fork, the public artifact it was made from, [forkedFrom] (its ID on
  /// the same remote). Throws [RemoteMoved] or [UsesAssets].
  Future<PushResult> publish(
    String aid, {
    required int version,
    String? rid,
    String? title,
    String? forkedFrom,
  });

  /// Attaches [bytes], the bundle of [lineage] at [bundleVersion] with
  /// manifest hash [contentSha256], to public version [remoteVersion] of
  /// [aid]'s public copy on [rid]. Sending the same bytes again succeeds.
  Future<void> putBundle(
    String aid,
    String rid,
    int remoteVersion,
    Uint8List bytes, {
    required String lineage,
    required int bundleVersion,
    required String contentSha256,
  });

  /// Links [aid] to [app] on [rid] without publishing, based on public
  /// version [remoteVersion]; a link [aid] has there already is based
  /// there again.
  Future<void> link(String rid, String aid, RemoteApp app, int remoteVersion);

  /// Ends [aid]'s link to its public copy on [rid] in the core; the copy
  /// stays. The next publish of [aid] there makes a new copy.
  Future<void> unlink(String aid, String rid);

  /// Posts [body] as a new comment on public copy [raid] on [rid], as the
  /// person signed in there. Refused 403 `comments_off` while the copy
  /// takes no comments, and 403 for an account that may only view it.
  Future<void> comment(String rid, String raid, String body);

  /// Public copy [raid]'s head on [rid], read with this device's token.
  Future<RemoteHead> head(String rid, String raid);

  /// The head of the public artifact at [artifactUrl] (`<origin>/a/<id>`),
  /// read without signing in.
  Future<RemoteHead> anonymousHead(Uri artifactUrl);

  /// The bundle public version [n] of [raid] on [rid] carries.
  Future<Uint8List> bundle(String rid, String raid, int n);

  /// The bundle at [url] (a head's `bundle.url`), read without signing in.
  Future<Uint8List> anonymousBundle(Uri url);

  /// Public version [n] of [raid] on [rid], file by file.
  Future<Map<String, Uint8List>> versionFiles(String rid, String raid, int n);

  /// The apps the account signed in to [rid] owns or holds a share on.
  Future<List<RemoteApp>> apps(String rid);

  /// [raid]'s sharing on [rid].
  Future<Sharing> sharing(String rid, String raid);

  /// Shares [raid] with [to] (a person on the remote, or a GitHub login)
  /// at [access] (`viewer`, `commenter` or `editor`), or changes their
  /// access.
  Future<void> share(String rid, String raid, ShareTarget to, String access);

  /// Stops sharing [raid] with share [sid].
  Future<void> unshare(String rid, String raid, String sid);

  /// Shares [raid] by link at [access] (`viewer` or `commenter`; null
  /// stops), [listed] in the public gallery or not.
  Future<void> setLink(
    String rid,
    String raid,
    String? access, {
    bool listed = false,
  });

  /// Turns comments on [raid] on or off.
  Future<void> setComments(String rid, String raid, bool on);

  /// Deletes the public copy [raid] on [rid] and unlinks the local app.
  Future<void> unpublish(String rid, String raid);

  /// Counts an install of the public artifact at [artifactUrl] by
  /// [installId] (64 hex), without signing in: the remote counts it only
  /// for a copy shared by link.
  Future<void> recordInstall(Uri artifactUrl, String installId);

  /// Counts an install of public copy [raid] on [rid] by [installId] (64
  /// hex) with the core's token for [rid], so a copy shared with the
  /// signed-in account by name counts it too.
  Future<void> countInstall(String rid, String raid, String installId);
}

/// [RemoteApi] over a [ClaxTransport] for the core's routes, and a
/// `dart:io` [HttpClient] for the remote's public routes.
///
/// A call with no answer throws `ClaxApiError` status 0
/// ([ClaxApiError.timeout] or [ClaxApiError.unreachable]); a refusal
/// throws its status and error; an answer of the wrong shape throws code
/// `bad_response`; a bundle over [bundleCap] throws 413
/// `bundle_too_large`.
class HttpRemoteApi implements RemoteApi {
  HttpRemoteApi(
    this.transport, {
    HttpClient Function()? httpClient,
    this.requestTimeout = const Duration(seconds: 35),
    this.bundleTimeout = const Duration(seconds: 125),
    this.bundleCap = maxBundleBytes,
  }) : httpClient = httpClient ?? HttpClient.new;

  final ClaxTransport transport;

  /// Makes the client of each call to a remote's public routes.
  final HttpClient Function() httpClient;
  final Duration requestTimeout;

  /// The limit for sending or fetching a bundle.
  final Duration bundleTimeout;

  /// The most bytes a fetched bundle may hold.
  final int bundleCap;

  @override
  Future<List<Remote>> remotes() async {
    final j = await _json('GET', '/api/remotes');
    return _parse(
      () => [
        for (final r in j['remotes'] as List<Object?>)
          Remote.fromJson((r as Map).cast()),
      ],
    );
  }

  @override
  Future<SignIn> addRemote(
    String url, {
    String kind = 'clax-app',
    String? machine,
  }) async {
    final j = await _json(
      'POST',
      '/api/remotes',
      body: {'url': url, 'kind': kind, 'machine': ?machine},
      person: true,
    );
    return _parse(() => SignIn.fromJson(j));
  }

  @override
  Future<Remote> poll(String rid) async {
    final j = await _json('POST', '/api/remotes/${_seg(rid)}/poll');
    return _parse(() {
      final r = Remote.fromJson((j['remote'] as Map).cast());
      return Remote(
        id: r.id,
        origin: r.origin,
        name: r.name,
        status: j['status'] as String? ?? r.status,
        personName: r.personName,
      );
    });
  }

  @override
  Future<bool> removeRemote(String rid) async {
    final j = await _json('DELETE', '/api/remotes/${_seg(rid)}');
    return j['revoked'] == true;
  }

  @override
  Future<PushResult> publish(
    String aid, {
    required int version,
    String? rid,
    String? title,
    String? forkedFrom,
  }) async {
    final Map<String, Object?> j;
    try {
      j = await _json(
        'POST',
        '/api/artifacts/${_seg(aid)}/push',
        body: {
          'mode': 'app',
          'version': version,
          'remote': ?rid,
          'title': ?title,
          'forked_from': ?forkedFrom,
        },
        timeout: bundleTimeout,
        person: true,
      );
    } on ClaxApiError catch (e) {
      if (e.status == 409 && e.code == 'remote_moved') {
        final at = e.body['remote_version'];
        if (at is num) throw RemoteMoved(at.toInt());
      }
      if (e.code == 'uses_assets') throw UsesAssets(e.message);
      rethrow;
    }
    return _parse(() => PushResult.fromJson(j));
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
    final r = await transport.send(
      'PUT',
      '/api/artifacts/${_seg(aid)}/remotes/${_seg(rid)}/bundle',
      query: {'remote_n': '$remoteVersion'},
      headers: {
        'content-type': bundleMime,
        'clax-bundle-lineage': lineage,
        'clax-bundle-version': '$bundleVersion',
        'clax-bundle-content-sha256': contentSha256,
      },
      body: bytes,
      timeout: bundleTimeout,
    );
    _check(r);
  }

  @override
  Future<void> link(
    String rid,
    String aid,
    RemoteApp app,
    int remoteVersion,
  ) async {
    await _json(
      'POST',
      '/api/remotes/${_seg(rid)}/links',
      body: {
        'artifact_id': aid,
        'remote_artifact_id': app.artifactId,
        'url': app.url,
        'remote_n': remoteVersion,
      },
    );
  }

  @override
  Future<void> unlink(String aid, String rid) async {
    await _json('DELETE', '/api/artifacts/${_seg(aid)}/remotes/${_seg(rid)}');
  }

  @override
  Future<void> comment(String rid, String raid, String body) async {
    await _json('POST', '${_copy(rid, raid)}/comments', body: {'body': body});
  }

  @override
  Future<RemoteHead> head(String rid, String raid) async {
    final j = await _json('GET', '${_copy(rid, raid)}/head');
    return _parse(() => RemoteHead.fromJson(j));
  }

  @override
  Future<RemoteHead> anonymousHead(Uri artifactUrl) async {
    final (origin, id) = _artifact(artifactUrl);
    final body = await _fetch(
      'GET',
      origin.replace(path: '/api/artifacts/${_seg(id)}/head'),
      cap: _jsonCap,
    );
    return _parse(() => RemoteHead.fromJson(_decodeObject(body)));
  }

  @override
  Future<Uint8List> bundle(String rid, String raid, int n) async {
    final r = await transport.send(
      'GET',
      '${_copy(rid, raid)}/versions/$n/bundle',
      timeout: bundleTimeout,
    );
    _check(r);
    if (r.body.length > bundleCap) throw _tooLarge();
    return r.body;
  }

  @override
  Future<Uint8List> anonymousBundle(Uri url) {
    if (!url.hasScheme || !url.hasAuthority) {
      throw ClaxApiError(
        status: 400,
        code: 'invalid_args',
        message: '$url is not an absolute address',
      );
    }
    return _fetch(
      'GET',
      url,
      cap: bundleCap,
      timeout: bundleTimeout,
      tooLarge: _tooLarge,
    );
  }

  @override
  Future<Map<String, Uint8List>> versionFiles(
    String rid,
    String raid,
    int n,
  ) async {
    final base = '${_copy(rid, raid)}/versions/$n/files';
    final j = await _json('GET', base);
    final paths = _parse(
      () => ((j['version'] as Map)['files'] as Map).keys.cast<String>().toList(),
    );
    final bodies = await Future.wait([
      for (final p in paths)
        transport
            .send('GET', '$base/${_path(p)}', timeout: requestTimeout)
            .then((r) {
              _check(r);
              return r.body;
            }),
    ]);
    return {for (var i = 0; i < paths.length; i++) paths[i]: bodies[i]};
  }

  @override
  Future<List<RemoteApp>> apps(String rid) async {
    final r = await _send('GET', '/api/remotes/${_seg(rid)}/apps');
    _check(r);
    return _parse(() {
      final j = r.json ?? jsonDecode(utf8.decode(r.body));
      final list = j is List ? j : (j as Map)['apps'] as List<Object?>;
      return [for (final a in list) RemoteApp.fromJson((a as Map).cast())];
    });
  }

  @override
  Future<Sharing> sharing(String rid, String raid) async {
    final j = await _json('GET', '${_copy(rid, raid)}/sharing');
    return _parse(() => Sharing.fromJson(j));
  }

  @override
  Future<void> share(
    String rid,
    String raid,
    ShareTarget to,
    String access,
  ) async {
    await _json(
      'PUT',
      '${_copy(rid, raid)}/sharing/shares',
      body: {...to.toJson(), 'access': access},
      person: true,
    );
  }

  @override
  Future<void> unshare(String rid, String raid, String sid) async {
    await _json('DELETE', '${_copy(rid, raid)}/sharing/shares/${_seg(sid)}');
  }

  @override
  Future<void> setLink(
    String rid,
    String raid,
    String? access, {
    bool listed = false,
  }) async {
    await _json(
      'PUT',
      '${_copy(rid, raid)}/sharing/link',
      body: {'access': access, 'listed': listed},
      person: true,
    );
  }

  @override
  Future<void> setComments(String rid, String raid, bool on) async {
    await _json(
      'PUT',
      '${_copy(rid, raid)}/sharing/comments',
      body: {'on': on},
      person: true,
    );
  }

  @override
  Future<void> unpublish(String rid, String raid) async {
    await _json('DELETE', _copy(rid, raid));
  }

  @override
  Future<void> countInstall(String rid, String raid, String installId) async {
    await _json(
      'POST',
      '${_copy(rid, raid)}/installs',
      body: {'install': installId},
    );
  }

  @override
  Future<void> recordInstall(Uri artifactUrl, String installId) async {
    final (origin, id) = _artifact(artifactUrl);
    await _fetch(
      'POST',
      origin.replace(path: '/api/artifacts/${_seg(id)}/installs'),
      json: {'install': installId},
      cap: _jsonCap,
    );
  }

  /// The most bytes a JSON answer from a remote may hold.
  static const _jsonCap = 1 << 20;

  static String _seg(String s) => Uri.encodeComponent(s);

  static String _path(String p) =>
      p.split('/').map(Uri.encodeComponent).join('/');

  /// The core's route for public copy [raid] on [rid].
  static String _copy(String rid, String raid) =>
      '/api/remotes/${_seg(rid)}/artifacts/${_seg(raid)}';

  /// [url]'s origin and artifact ID, for `<origin>/a/<id>`; throws 400
  /// `invalid_args` for any other address.
  static (Uri, String) _artifact(Uri url) {
    final segs = [...url.pathSegments];
    if (segs.isNotEmpty && segs.last.isEmpty) segs.removeLast();
    if ((url.scheme != 'https' && url.scheme != 'http') ||
        !url.hasAuthority ||
        segs.length != 2 ||
        segs[0] != 'a' ||
        segs[1].isEmpty) {
      throw ClaxApiError(
        status: 400,
        code: 'invalid_args',
        message: '$url is not a public artifact\'s address',
      );
    }
    return (Uri(scheme: url.scheme, host: url.host, port: url.port), segs[1]);
  }

  static ClaxApiError _tooLarge() => const ClaxApiError(
    status: 413,
    code: 'bundle_too_large',
    message: 'the bundle is over 32 MiB',
  );

  /// A JSON answer from [url]'s origin over [_jsonCap]: not one Clax can
  /// read, whatever the host meant by it.
  static ClaxApiError _answerTooLarge(Uri url) => ClaxApiError(
    status: 200,
    code: 'bad_response',
    message: 'the answer from ${url.origin} is over ${_jsonCap >> 20} MiB',
  );

  static T _parse<T>(T Function() read) {
    try {
      return read();
    } on TypeError catch (e) {
      throw ClaxApiError(status: 200, code: 'bad_response', message: '$e');
    } on FormatException catch (e) {
      throw ClaxApiError(status: 200, code: 'bad_response', message: e.message);
    }
  }

  static Map<String, Object?> _decodeObject(Uint8List body) {
    if (body.isEmpty) return {};
    final j = jsonDecode(utf8.decode(body));
    if (j is Map) return j.cast<String, Object?>();
    throw const FormatException('the answer is not a JSON object');
  }

  /// Throws the error of an answer that is not 2xx.
  static void _check(TransportAnswer r) => _refusal(r.status, r.body);

  static void _refusal(int status, Uint8List body) {
    if (status >= 200 && status < 300) return;
    Object? decoded;
    try {
      decoded = jsonDecode(utf8.decode(body, allowMalformed: true));
    } on FormatException catch (_) {}
    var err = const <String, Object?>{};
    if (decoded is Map && decoded['error'] is Map) {
      err = (decoded['error'] as Map).cast<String, Object?>();
    }
    throw ClaxApiError(
      status: status,
      code: err['code'] as String? ?? 'http_$status',
      message: err['message'] as String? ?? 'HTTP $status',
      body: err,
    );
  }

  /// [method] [path] with [body] as JSON. With [person] the call is the
  /// person's own decision, made on the app's screen (connecting, pushing,
  /// sharing, widening a public copy's access): it speaks as the person,
  /// as their browser does ([ClaxTransport.send]).
  Future<TransportAnswer> _send(
    String method,
    String path, {
    Object? body,
    Duration? timeout,
    bool person = false,
  }) => transport.send(
    method,
    path,
    headers: {if (body != null) 'content-type': 'application/json'},
    body: body == null ? null : utf8.encode(jsonEncode(body)),
    timeout: timeout ?? requestTimeout,
    person: person,
  );

  Future<Map<String, Object?>> _json(
    String method,
    String path, {
    Object? body,
    Duration? timeout,
    bool person = false,
  }) async {
    final r = await _send(
      method,
      path,
      body: body,
      timeout: timeout,
      person: person,
    );
    _check(r);
    if (r.json case final j?) return j;
    return _parse(() => _decodeObject(r.body));
  }

  /// [method] [url] straight to a remote, without signing in: the 2xx
  /// answer's body, at most [cap] bytes; a longer one throws [tooLarge]'s
  /// error ([_answerTooLarge]'s when null, for a JSON answer).
  Future<Uint8List> _fetch(
    String method,
    Uri url, {
    Object? json,
    required int cap,
    Duration? timeout,
    ClaxApiError Function()? tooLarge,
  }) async {
    final client = httpClient();
    try {
      return await _exchange(
        client,
        method,
        url,
        json,
        cap,
        tooLarge ?? () => _answerTooLarge(url),
      ).timeout(timeout ?? requestTimeout);
    } on TimeoutException {
      throw ClaxApiError(
        status: 0,
        code: ClaxApiError.timeout,
        message: 'no answer from ${url.origin}',
      );
    } on IOException catch (e) {
      throw ClaxApiError(
        status: 0,
        code: ClaxApiError.unreachable,
        message: '${url.origin} could not be reached',
        body: {'reason': '$e'},
      );
    } finally {
      client.close(force: true);
    }
  }

  Future<Uint8List> _exchange(
    HttpClient client,
    String method,
    Uri url,
    Object? json,
    int cap,
    ClaxApiError Function() tooLarge,
  ) async {
    final req = await client.openUrl(method, url);
    req.followRedirects = false;
    if (json != null) {
      req.headers.contentType = ContentType.json;
      req.add(utf8.encode(jsonEncode(json)));
    }
    final res = await req.close();
    final out = BytesBuilder(copy: false);
    await for (final chunk in res) {
      out.add(chunk);
      if (out.length > cap) {
        _refusal(res.statusCode, out.takeBytes());
        throw tooLarge();
      }
    }
    final body = out.takeBytes();
    _refusal(res.statusCode, body);
    return body;
  }
}

/// Who [RemoteApi.share] shares with: a person on the remote, or a GitHub
/// login (the body `put_share` takes, as the web dialog sends it).
sealed class ShareTarget {
  const ShareTarget();

  /// The request body's `person` or `github_login`.
  Map<String, Object?> toJson();
}

/// A person on the remote, by public ID: someone with no GitHub login
/// (joined by invite) is shared with this way.
final class SharePerson extends ShareTarget {
  const SharePerson(this.person);

  final String person;

  @override
  Map<String, Object?> toJson() => {'person': person};
}

/// A GitHub login, an `@` first dropped.
final class ShareLogin extends ShareTarget {
  ShareLogin(String login)
    : login = login.startsWith('@') ? login.substring(1) : login;

  final String login;

  @override
  Map<String, Object?> toJson() => {'github_login': login};
}
