import 'dart:typed_data';

import 'backend.dart' show AgentError, AgentErrorKind;
import 'progress.dart';

/// What a request to the agent asks for.
enum RequestKind {
  /// Make a new app from the person's description.
  newApp,

  /// Change an existing app in answer to comments on it.
  change,
}

/// One comment the agent answers in a change.
class CommentContext {
  const CommentContext({
    required this.threadId,
    required this.author,
    required this.body,
    required this.anchorLine,
    this.clip,
  });

  final String threadId;
  final String author;

  /// The person's words. The prompt carries them as a JSON string, as data.
  final String body;

  /// What the comment is anchored on: the selector or "the whole
  /// artifact", the quoted text and the version. The prompt carries it as a
  /// JSON string after "Anchored on: ", since its quote is page text.
  final String anchorLine;

  /// The PNG clip of the anchored region. Set it on the first comment only
  /// when the request's backend sends `AgentRequest.image`: the prompt then
  /// says that the attached image is this comment's clip.
  final Uint8List? clip;
}

/// One collection of another app's documents, with up to two samples.
typedef OtherCollection = ({
  String path,
  int count,
  List<Map<String, Object?>> samples,
});

/// Another of the owner's apps whose documents a new app or a change may
/// read with `db.artifact(id)`.
class OtherApp {
  const OtherApp({
    required this.id,
    required this.title,
    required this.collections,
  });

  final String id;
  final String title;
  final List<OtherCollection> collections;

  /// The form the prompt carries: `{id, title, collections: [{path, count,
  /// samples}]}`.
  Map<String, Object?> toJson() => {
    'id': id,
    'title': title,
    'collections': [
      for (final c in collections)
        {'path': c.path, 'count': c.count, 'samples': c.samples},
    ],
  };
}

/// One call to a backend.
class AgentRequest {
  const AgentRequest({
    required this.kind,
    required this.model,
    required this.systemPrompt,
    required this.userText,
    this.image,
    this.cancelled,
    this.onProgress,
  });

  final RequestKind kind;

  /// The model ID, for example `claude-haiku-5-5`.
  final String model;
  final String systemPrompt;
  final String userText;

  /// The first comment's clip (PNG), for backends that take images.
  final Uint8List? image;

  /// Completes (with a value or an error) when the caller no longer wants
  /// the result. The backend then stops its work (ends the process, aborts
  /// the HTTP request) and throws an [AgentError] of kind
  /// [AgentErrorKind.cancelled]. Null: the request is never cancelled.
  final Future<void>? cancelled;

  /// Receives [AgentProgress] events while the run is under way. A backend
  /// that can stream its answer does so only when this is set; null runs
  /// the request without streaming. Events stop once [cancelled] fires.
  final ProgressSink? onProgress;
}
