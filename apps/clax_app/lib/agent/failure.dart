/// The reply the app's agent posts on a thread whose change it could not
/// make, and how the app recognises it.
///
/// Every producer of the reply writes it with [failureReply], and every
/// reader (the worker's retry, the thread page) recognises it with
/// [isAppFailureReply], so rewording it changes both together.
library;

import '../clax/models.dart' show ThreadComment;

/// How a failure reply opens.
const failurePrefix = "Couldn't make that change:";

/// The same opening with a typographic apostrophe, as a person or a
/// keyboard may have typed it.
const _curlyPrefix = 'Couldn’t make that change:';

/// The app session's agent harness (`via_harness` on its comments).
const appHarness = 'app';

/// The failure reply for [reason]: "Couldn't make that change: [reason]".
String failureReply(String reason) => '$failurePrefix $reason';

/// Whether [body] opens as a failure reply does.
bool isFailureText(String body) =>
    body.startsWith(failurePrefix) || body.startsWith(_curlyPrefix);

/// Whether [c] is a failure reply of this app's agent: an agent comment
/// posted by the app session (`via_harness` `app`) that opens as
/// [failureReply] does. A reply from another harness, or a person's
/// comment with the same words, is not one.
bool isAppFailureReply(ThreadComment c) =>
    c.byAgent && c.viaHarness == appHarness && isFailureText(c.body);
