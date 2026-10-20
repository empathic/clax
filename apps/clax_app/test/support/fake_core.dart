import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:isolate';
import 'dart:typed_data';

import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/core/core_client.dart';
import 'package:clax_app/core/ffi.dart';
import 'package:clax_app/core/frame.dart';

/// Answers a [FakeCore] request; [params] holds the path's `{name}`
/// segments, decoded.
typedef CoreHandler =
    FutureOr<CoreResponse> Function(
      CoreRequest req,
      Map<String, String> params,
    );

/// An in-memory core for tests: [call] dispatches on the method and the
/// path (without its query) to the handlers a test registers with [on],
/// and answers 404 `not_in_fake` for any other; [subscribe] hands out
/// subscriptions whose blocks a test [push]es. Every request is appended
/// to [requests] before it is answered, and every subscription's resume
/// point to [resumes].
class FakeCore implements CoreCalls {
  FakeCore();

  /// A core whose routes are [api]'s, in the JSON shapes `HttpClaxApi`
  /// reads: `GET /api/artifacts`, `GET /api/artifacts/{aid}`,
  /// `POST /api/artifacts`, `POST /api/artifacts/{aid}/versions`,
  /// `POST /api/sessions`, `GET /api/sessions/{sid}/feedback`,
  /// `POST /api/sessions/{sid}/feedback/ack`,
  /// `POST /api/artifacts/{aid}/threads/{tid}/comments` (the owner's, or
  /// an agent's with `author_kind: agent`),
  /// `POST /api/artifacts/{aid}/docs:batch` and `POST /api/stream/{id}`.
  /// A [ClaxApiError] answers its status and `{"error": …}`.
  factory FakeCore.fromApi(FakeClaxApi api) => FakeCore().._routeTo(api);

  final List<CoreRequest> requests = [];

  /// The `lastEventId` of each [subscribe], in order.
  final List<String?> resumes = [];

  final List<(String, List<String>, CoreHandler)> _routes = [];
  final List<FakeSubscription> _subs = [];

  /// Answers [method] on [path], whose segments may be `{name}`.
  void on(String method, String path, CoreHandler handler) =>
      _routes.add((method, _segments(path), handler));

  /// The subscriptions not yet ended or cancelled.
  int get subscribers => _subs.where((s) => s.live).length;

  /// Sends [block] to every live subscription.
  void push(String block) {
    for (final s in _subs) {
      s.push(block);
    }
  }

  /// Ends every live subscription with an `end` block naming [reason].
  void end(String reason) {
    for (final s in _subs) {
      s.end(reason);
    }
  }

  @override
  Future<CoreResponse> call(CoreRequest r) async {
    requests.add(r);
    final path = _segments(Uri.parse(r.path).path);
    for (final (method, pattern, handler) in _routes) {
      if (method != r.method) continue;
      final params = _match(pattern, path);
      if (params == null) continue;
      try {
        return await handler(r, params);
      } on ClaxApiError catch (e) {
        return error(e.status, e.code, e.message, e.body);
      }
    }
    return error(404, 'not_in_fake', 'FakeCore answers no ${r.method} ${r.path}');
  }

  @override
  CoreSubscription subscribe({String? lastEventId}) {
    resumes.add(lastEventId);
    final s = FakeSubscription();
    _subs.add(s);
    return s;
  }

  /// A JSON answer.
  static CoreResponse json(Object? body, {int status = 200}) => CoreResponse(
    status,
    const [('content-type', 'application/json')],
    utf8.encode(jsonEncode(body)),
  );

  /// The core's error answer: `{"error": {code, message, …extra}}`.
  static CoreResponse error(
    int status,
    String code, [
    String message = '',
    Map<String, Object?> extra = const {},
  ]) => json({
    'error': {...extra, 'code': code, 'message': message},
  }, status: status);

  /// [r]'s body as JSON (null when it has none).
  static Object? bodyOf(CoreRequest r) =>
      r.body == null || r.body!.isEmpty ? null : jsonDecode(utf8.decode(r.body!));

  static List<String> _segments(String path) =>
      path.split('/').where((s) => s.isNotEmpty).toList();

  static Map<String, String>? _match(List<String> pattern, List<String> path) {
    if (pattern.length != path.length) return null;
    final params = <String, String>{};
    for (var i = 0; i < pattern.length; i++) {
      final p = pattern[i];
      if (p.startsWith('{') && p.endsWith('}')) {
        params[p.substring(1, p.length - 1)] = Uri.decodeComponent(path[i]);
      } else if (p != path[i]) {
        return null;
      }
    }
    return params;
  }

  void _routeTo(FakeClaxApi api) {
    Map<String, Object?> body(CoreRequest r) =>
        (bodyOf(r) as Map?)?.cast<String, Object?>() ?? {};
    on('GET', '/api/artifacts', (r, _) async {
      final list = await api.listArtifacts();
      return json({'artifacts': [for (final a in list) _summary(a)]});
    });
    on('GET', '/api/artifacts/{aid}', (r, p) async {
      final d = await api.getArtifact(p['aid']!);
      return json({
        'artifact': _summary(d.artifact),
        'versions': [for (final v in d.versions) _version(v)],
      });
    });
    on('POST', '/api/artifacts', (r, _) async {
      return json(_published(await api.createArtifact(_publishRequest(body(r)))));
    });
    on('POST', '/api/artifacts/{aid}/versions', (r, p) async {
      return json(
        _published(await api.publish(p['aid']!, _publishRequest(body(r)))),
      );
    });
    on('POST', '/api/sessions', (r, _) async {
      final b = body(r);
      final s = await api.registerAppSession(
        installId: b['harness_session_id']! as String,
        pid: (b['pid']! as num).toInt(),
        cwd: b['cwd'] as String? ?? '',
      );
      return json({
        'session': {
          'id': s.id,
          'harness': s.harness,
          'harness_session_id': s.harnessSessionId,
          'cwd': s.cwd,
          'pid': s.pid,
          'agent_handle': s.agentHandle,
          'ended_at': s.endedAt,
        },
      });
    });
    on('GET', '/api/sessions/{sid}/feedback', (r, p) async {
      final wait = Uri.parse(r.path).queryParameters['wait'];
      final batch = await api.pollFeedback(
        p['sid']!,
        wait: Duration(seconds: int.parse(wait ?? '50')),
      );
      if (batch.answers.isNotEmpty) {
        throw UnimplementedError('FakeCore.fromApi does not encode late answers');
      }
      return json({
        'feedback': [for (final f in batch.items) _feedback(f)],
        'text': batch.text,
        'answers': const <Object?>[],
      });
    });
    on('POST', '/api/sessions/{sid}/feedback/ack', (r, p) async {
      final ids = [for (final c in body(r)['comment_ids']! as List) c as String];
      await api.ackFeedback(p['sid']!, ids);
      return json({});
    });
    on('POST', '/api/artifacts/{aid}/threads/{tid}/comments', (r, p) async {
      final b = body(r);
      if (b['author_kind'] == 'agent') {
        await api.replyAsAgent(p['aid']!, p['tid']!, b['body']! as String);
        return json({});
      }
      final t = await api.postOwnerComment(
        p['aid']!,
        p['tid']!,
        b['body']! as String,
      );
      return json({'thread': _thread(t)});
    });
    on('POST', '/api/artifacts/{aid}/docs:batch', (r, p) async {
      final writes = [
        for (final w in body(r)['writes']! as List)
          _docWrite((w as Map).cast()),
      ];
      await api.batchDocs(p['aid']!, writes);
      return json({});
    });
    on('POST', '/api/stream/{id}', (r, _) => json({}));
  }

  static Map<String, Object?> _summary(ArtifactSummary a) => {
    'id': a.id,
    'title': a.title,
    'description': a.description,
    'icon': a.icon,
    'created_at': a.createdAt,
    'updated_at': a.updatedAt,
    'current_version': a.currentVersion,
    'pinned': a.pinned,
    'capabilities': a.capabilities,
    'owner_session_id': a.ownerSessionId,
    'owner_live': a.ownerLive,
    'owner_harness': a.ownerHarness,
    'kind': a.kind,
  };

  static Map<String, Object?> _version(VersionInfo v) => {
    'artifact_id': v.artifactId,
    'n': v.n,
    'label': v.label,
    'created_at': v.createdAt,
    'files': {
      for (final e in v.files.entries)
        e.key: {
          'content_type': e.value.contentType,
          'size': e.value.size,
          'sha256': e.value.sha256,
        },
    },
    'note': v.note,
    'addresses': v.addresses,
    'session_id': v.sessionId,
    'agent': v.agent,
    'agent_harness': v.agentHarness,
    'content_sha256': v.contentSha256,
  };

  static Map<String, Object?> _published(PublishResult r) => {
    'artifact': _summary(r.artifact),
    'version': _version(r.version),
    'url': r.url,
    'note_truncated': r.noteTruncated,
  };

  static Map<String, Object?> _feedback(FeedbackItem f) => {
    'feedback_id': f.feedbackId,
    'thread_id': f.threadId,
    'comment_id': f.commentId,
    'artifact_id': f.artifactId,
    'artifact_title': f.artifactTitle,
    'body': f.body,
    'author': f.author,
    'version': f.version,
    'anchor': f.anchor,
    'clip_path': f.clipPath,
    'via_page': f.viaPage,
    'resent': f.resent,
    'created_at': f.createdAt,
    if (f.batchNote != null) 'batch': {'note': f.batchNote},
  };

  static Map<String, Object?> _thread(ThreadView t) => {
    'id': t.id,
    'artifact_id': t.artifactId,
    'version_n': t.versionN,
    'anchor': t.anchor,
    'status': t.status,
    'sent_to_agent': t.sentToAgent,
    'has_clip': t.hasClip,
    'created_at': t.createdAt,
    'resolved_at': t.resolvedAt,
    'resolved_by': t.resolvedBy,
    'comments': [
      for (final c in t.comments)
        {
          'id': c.id,
          'author_kind': c.authorKind,
          'author_name': c.authorName,
          'body': c.body,
          'created_at': c.createdAt,
          'via_harness': c.viaHarness,
          'via_page': c.viaPage,
          'author_public_id': c.authorPublicId,
        },
    ],
    'feedback_state': switch (t.feedbackState) {
      null => null,
      final f => {
        'state': f.state,
        'tier': f.tier,
        'since': f.since,
        'resends': f.resends,
        'exhausted': f.exhausted,
      },
    },
    'addressed_in': t.addressedIn,
  };

  /// A publish body (`PublishRequest.toJson`) read back.
  static PublishRequest _publishRequest(Map<String, Object?> j) =>
      PublishRequest(
        title: j['title'] as String?,
        icon: j['icon'] as String?,
        label: j['label'] as String?,
        note: j['note'] as String?,
        ifVersion: (j['if_version'] as num?)?.toInt(),
        capabilities: (j['capabilities'] as Map?)?.cast(),
        entries: {
          for (final e in ((j['files'] as Map?) ?? const {}).entries)
            e.key as String: switch (e.value) {
              null => null,
              final Map f when f['encoding'] == 'base64' => FileEntry.bytes(
                base64Decode(f['content'] as String),
                type: f['content_type'] as String?,
              ),
              final Map f => FileEntry.text(
                f['content'] as String,
                type: f['content_type'] as String?,
              ),
              final v => throw FormatException('a file entry: $v'),
            },
        },
        addresses: [for (final a in (j['addresses'] as List?) ?? const []) a as String],
      );

  /// A `docs:batch` write (`DocWrite.toJson`) read back.
  static DocWrite _docWrite(Map<String, Object?> j) {
    final path = j['path']! as String;
    final at = path.lastIndexOf('/');
    return DocWrite(
      op: j['op']! as String,
      collection: path.substring(0, at),
      docId: path.substring(at + 1),
      data: (j['data'] as Map?)?.cast(),
      ifVersion: (j['if_version'] as num?)?.toInt(),
    );
  }
}

/// A [FakeCore] subscription: the blocks [push]ed until [end] or [cancel].
class FakeSubscription implements CoreSubscription {
  final StreamController<String> _out = StreamController();

  /// Whether it still takes blocks.
  bool get live => !_out.isClosed;

  @override
  Stream<String> get blocks => _out.stream;

  void push(String block) {
    if (live) _out.add(block);
  }

  /// Sends the `end` block naming [reason] and ends.
  void end(String reason) {
    if (!live) return;
    _out.add(endBlock(reason));
    _out.close();
  }

  @override
  void cancel() {
    if (live) _out.close();
  }
}

/// One [FakeCoreLib.open]: the home, the port its states go to, and what
/// `<home>/embed.json` held when it was called (null when absent).
typedef FakeOpen = ({String home, SendPort port, String? embedJson});

/// One [FakeCoreLib.call]: the handle, the request, and where its answer
/// goes.
typedef FakeLibCall = ({int core, CoreRequest request, SendPort port, int tag});

/// One [FakeCoreLib.subscribe].
typedef FakeLibSubscription = ({int core, SendPort port});

/// The four C functions in Dart, for [CoreClient]'s tests: each call is
/// recorded and answered with the test's code (`openAnswer`, …), and
/// messages reach the client's ports as the core would post them
/// ([postState], [answer], [postBlock]).
class FakeCoreLib implements CoreLib {
  int openAnswer = 5;
  int callAnswer = 0;
  int subscribeAnswer = 0;
  int closeAnswer = 0;

  final List<FakeOpen> opens = [];
  final List<FakeLibCall> calls = [];
  final List<FakeLibSubscription> subscriptions = [];
  final List<int> closes = [];

  @override
  int open(String home, SendPort port) {
    final embed = File('$home/embed.json');
    opens.add((
      home: home,
      port: port,
      embedJson: embed.existsSync() ? embed.readAsStringSync() : null,
    ));
    return openAnswer;
  }

  @override
  int call(int core, Uint8List req, SendPort port, int tag) {
    if (callAnswer == 0) {
      calls.add((core: core, request: decodeRequest(req), port: port, tag: tag));
    }
    return callAnswer;
  }

  @override
  int subscribe(int core, SendPort port) {
    if (subscribeAnswer == 0) subscriptions.add((core: core, port: port));
    return subscribeAnswer;
  }

  @override
  int close(int core) {
    closes.add(core);
    return closeAnswer;
  }

  /// Posts [state] as JSON to the port of the [open]th open.
  void postState(Map<String, Object?> state, {int open = 0}) =>
      opens[open].port.send(utf8.encode(jsonEncode(state)));

  /// Posts [c]'s answer: its tag, then a response frame.
  void answer(
    FakeLibCall c,
    int status, {
    List<(String, String)> headers = const [],
    String body = '',
  }) {
    final frame = responseFrame(status, headers, utf8.encode(body));
    c.port.send(
      Uint8List.fromList([
        ...(ByteData(8)..setInt64(0, c.tag, Endian.little)).buffer
            .asUint8List(),
        ...frame,
      ]),
    );
  }

  /// Posts one SSE block to [s].
  void postBlock(FakeLibSubscription s, String block) =>
      postBytes(s, utf8.encode(block));

  void postBytes(FakeLibSubscription s, List<int> bytes) =>
      s.port.send(Uint8List.fromList(bytes));
}

/// A response frame as crates/clax-embed/src/frame.rs encodes one.
Uint8List responseFrame(
  int status,
  List<(String, String)> headers,
  List<int> body,
) {
  final head = utf8.encode(
    jsonEncode({
      'status': status,
      'headers': [
        for (final (k, v) in headers) [k, v],
      ],
    }),
  );
  return Uint8List.fromList([
    ...ascii.encode('CLX1'),
    ...(ByteData(4)..setUint32(0, head.length, Endian.little)).buffer
        .asUint8List(),
    ...head,
    ...body,
  ]);
}

/// A request frame read back, as crates/clax-embed/src/frame.rs reads one.
CoreRequest decodeRequest(Uint8List frame) {
  final n = ByteData.sublistView(frame).getUint32(4, Endian.little);
  final head = (jsonDecode(utf8.decode(frame.sublist(8, 8 + n))) as Map)
      .cast<String, Object?>();
  final body = frame.sublist(8 + n);
  return CoreRequest(
    head['method']! as String,
    head['path']! as String,
    headers: [
      for (final h in (head['headers'] as List?) ?? const [])
        ((h as List)[0] as String, h[1] as String),
    ],
    body: body.isEmpty ? null : body,
    as: CallAs.values.firstWhere((a) => a.wire == head['as']),
    host: head['host'] as String?,
  );
}
