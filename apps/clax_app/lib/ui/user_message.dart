import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';

import '../agent/backend.dart';
import '../clax/api.dart';
import 'app_surface.dart';

/// One short line for [e], as the person reads it: what happened, and what
/// they can do (spec §7.3). Never a type name, a code or stack text.
///
/// - A socket or HTTP failure, or a page that failed with `net::ERR_…`:
///   Clax’s service didn’t answer.
/// - A timeout: it took too long.
/// - The Claude API overloaded (529) or rate limited (429): Claude is busy.
/// - The Claude API refusing the key (401, `authentication_error`): check
///   the key in Settings.
/// - An [AgentError] of [AgentErrorKind.network], or one saying the Claude
///   API couldn't be reached or the connection to it was lost or stalled:
///   Claude couldn't be reached.
/// - An [AgentError] saying the Claude API or `claude` ran out of time:
///   it took too long.
/// - An [AgentError] that carries a backend's raw report (an HTTP status
///   or the API's error type, `claude`'s own error or exit status, an
///   answer that was not a whole message or not an edit, or another
///   exception's text): Claude couldn't answer.
/// - Another [AgentError]: its own message, which is plain.
/// - A decision the core holds for the owner's browser confirmation
///   ([ClaxApiError.isConfirmationPending]): confirm it in the inbox.
/// - Anything else: something went wrong.
///
/// The raw text stays for the log and Developer tools ([rawError]).
///
/// A [ClaxApiError] is the service’s own answer, so its status alone says
/// nothing about Claude: it reads as busy or a refused key only when its
/// code says so (the service’s 429 `limit_reached` is not Claude being
/// busy).
String userMessage(Object e) {
  if (e is SocketException || e is HttpException) return _noAnswer;
  if (e is TimeoutException) return _tooLong;
  if (e is SurfaceFault) {
    return e.detail.startsWith('net::ERR_') ? _noAnswer : _unknown;
  }
  if (e is ClaxApiError) {
    if (e.isConfirmationPending) return _confirm;
    final code = e.code.toLowerCase();
    if (e.status == 529 || _busyCode.hasMatch(code)) return _busy;
    if (code == 'authentication_error') return _badKey;
    return _unknown;
  }
  final text = e is AgentError ? e.message : '$e'; // copy-check: allow
  if (e is AgentError && e.kind == AgentErrorKind.cancelled) return e.message;
  if (_busyText.hasMatch(text)) return _busy;
  if (_keyText.hasMatch(text)) return _badKey;
  if (e is AgentError) return _agentLine(e, text);
  if (text.contains('net::ERR_')) return _noAnswer;
  return _unknown;
}

/// The line for agent failure [e], whose message is [text]: busy and a
/// refused key are told before it.
String _agentLine(AgentError e, String text) {
  if (e.kind == AgentErrorKind.network || _agentUnreachable.hasMatch(text)) {
    return _unreachable;
  }
  if (_agentTimedOut.hasMatch(text)) return _tooLong;
  if (_agentRaw.hasMatch(text)) return _noReply;
  return text;
}

/// The raw text of [e], for the log and, with Developer tools on, Details.
String rawError(Object e) => switch (e) {
  SurfaceFault(:final detail) => detail,
  _ => '$e', // copy-check: allow
};

/// Logs [e]'s raw text under [where] with the app's logger.
void logError(String where, Object e) =>
    debugPrint('clax $where: ${rawError(e)}');

/// Logs [e]'s raw text under [where] with the app's logger, and answers
/// [userMessage] for the screen.
String logUserMessage(String where, Object e) {
  logError(where, e);
  return userMessage(e);
}

const _noAnswer = 'Clax’s service didn’t answer.';
const _tooLong = 'That took too long. Try again.';
const _busy = 'Claude is busy right now. Try again in a minute.';
const _badKey = 'Your Claude API key wasn’t accepted. Check it in Settings.';
const _unknown = 'Something went wrong. Try again.';
const _confirm = 'Confirm this in your inbox, then try again.';
const _unreachable =
    'Claude couldn’t be reached. Check your connection and try again.';
const _noReply = 'Claude couldn’t answer just now. Try again.';

final _busyCode = RegExp('overloaded|rate_limit');
final _busyText = RegExp(
  r'overloaded|rate[ _-]?limit|\b(429|529)\b',
  caseSensitive: false,
);
final _keyText = RegExp(
  r'authentication_error|api key was refused|invalid x-api-key|\(HTTP 401\)',
  caseSensitive: false,
);

/// An agent failure that says the Claude API was not reached, as
/// MessagesApiBackend and NetworkPause write it (also read back from a
/// failure reply, which keeps no kind).
final _agentUnreachable = RegExp(
  r"^Couldn't reach the Claude API|" // copy-check: allow
  r'^The connection to the Claude API (was lost|stalled)',
);

/// An agent failure that says the model's answer took too long.
final _agentTimedOut = RegExp(
  r'^The Claude API timed out|^claude did not finish within',
);

/// An agent failure that carries a backend's raw report rather than a
/// line for the person: MessagesApiBackend's HTTP and stream errors and
/// unreadable answers, ClaudeCliBackend's reports of `claude`, and
/// another exception's text wrapped as an AgentError.
final _agentRaw = RegExp(
  r'^Claude API error|'
  r'^The Claude API answered with something other than a message|'
  r'^The answer (was not a whole message|stopped before it finished|'
  r'had no text|broke off in a call)|'
  r'^The result (was not a JSON object|was malformed)|'
  r"^claude[ ']|^Couldn't start claude|" // copy-check: allow
  r'^(Bad state|Invalid argument)|^\w*(Exception|Error)\b|HTTP \d{3}',
);
