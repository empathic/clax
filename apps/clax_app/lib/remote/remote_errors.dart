/// The person's sentences for a failure of a call that reaches a hosted
/// Clax through the core's remote routes (docs/contract.md, the remote
/// error codes): one form per failure, wherever the app shows it.
library;

import '../clax/api.dart' show ClaxApiError;

/// Whether [e] says the remote could not be reached: no answer at all
/// (status 0), the core's 502 `remote_unreachable`, or the remote's own
/// 5xx passed on as 502 `remote_refused`.
bool remoteUnreachable(ClaxApiError e) =>
    e.unanswered ||
    e.code == 'remote_unreachable' ||
    (e.code == 'remote_refused' && (remoteStatus(e) ?? 0) >= 500);

/// Whether [e] is the core's 502 `remote_signed_out`: the remote no longer
/// takes this device's token (revoked there, or the account gone).
bool remoteSignedOut(ClaxApiError e) => e.code == 'remote_signed_out';

/// Whether [e] is the core's 502 `remote_too_old`: the hosted Clax is too
/// old for what was asked of it (a field it refuses, a route it lacks).
bool remoteTooOld(ClaxApiError e) => e.code == 'remote_too_old';

/// The remote's own status in the core's 502 `remote_refused`, when it
/// names one.
int? remoteStatus(ClaxApiError e) => switch (e.body['remote_status']) {
  final num n => n.toInt(),
  _ => null,
};

/// The line for a remote at [host] that could not be reached.
String unreachableLine(String host) =>
    'Couldn’t reach $host. Check your connection and try again.';

/// The line for a device [host] no longer takes the token of.
String signedOutLine(String host) =>
    'This device is signed out of $host. Connect again in Settings › '
    'Sharing.';

/// The line for a hosted Clax at [host] too old to [action] (a verb phrase
/// that follows "can": "sign in", "publish this app").
String tooOldLine(String host, String action) =>
    '$host needs an update before the Clax app can $action.';

/// The line for [e] from a call to [host] that tried to [action] (as
/// [tooOldLine] takes it) when it is about the remote as a whole: out of
/// reach, signed out, or too old; null for any other failure, which the
/// caller words.
String? remoteLine(
  ClaxApiError e, {
  required String host,
  required String action,
}) {
  if (remoteUnreachable(e)) return unreachableLine(host);
  if (remoteSignedOut(e)) return signedOutLine(host);
  if (remoteTooOld(e)) return tooOldLine(host, action);
  return null;
}

/// [remoteLine] for [e], else [otherwise].
String remoteFailureLine(
  ClaxApiError e, {
  required String host,
  required String action,
  required String otherwise,
}) => remoteLine(e, host: host, action: action) ?? otherwise;

/// The person's sentence for [e] from a call to [host] that tried to
/// [action]: [remoteLine]'s, else one for the refusals that say what is
/// wrong (an account not allowed, comments off, a bundle too large, an
/// answer that could not be read), else [otherwise]. The raw message is
/// for the log and Details, never this line.
String remoteErrorLine(
  ClaxApiError e, {
  required String host,
  required String action,
  required String otherwise,
}) {
  if (remoteLine(e, host: host, action: action) case final line?) return line;
  return switch (e.code) {
    'not_permitted' => 'This account isn’t allowed to do that on $host.',
    'comments_off' => 'Comments are off on this app.',
    'bundle_too_large' =>
      'This app is too large to share: its file is over 32 MiB.',
    'bad_response' => '$host sent an answer Clax couldn’t read.',
    _ => otherwise,
  };
}
