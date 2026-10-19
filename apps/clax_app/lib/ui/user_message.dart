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
/// - Another [AgentError]: its own message, which is already plain.
/// - Anything else: something went wrong.
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
    final code = e.code.toLowerCase();
    if (e.status == 529 || _busyCode.hasMatch(code)) return _busy;
    if (code == 'authentication_error') return _badKey;
    return _unknown;
  }
  final text = e is AgentError ? e.message : '$e';
  if (_busyText.hasMatch(text)) return _busy;
  if (_keyText.hasMatch(text)) return _badKey;
  if (e is AgentError) return e.message;
  if (text.contains('net::ERR_')) return _noAnswer;
  return _unknown;
}

/// The raw text of [e], for the log and, with Developer tools on, Details.
String rawError(Object e) => switch (e) {
  SurfaceFault(:final detail) => detail,
  _ => '$e',
};

/// Logs [e]'s raw text under [where] with the app's logger, and answers
/// [userMessage] for the screen.
String logUserMessage(String where, Object e) {
  debugPrint('clax $where: ${rawError(e)}');
  return userMessage(e);
}

const _noAnswer = 'Clax’s service didn’t answer.';
const _tooLong = 'That took too long. Try again.';
const _busy = 'Claude is busy right now. Try again in a minute.';
const _badKey = 'Your Claude API key wasn’t accepted. Check it in Settings.';
const _unknown = 'Something went wrong. Try again.';

final _busyCode = RegExp('overloaded|rate_limit');
final _busyText = RegExp(
  r'overloaded|rate[ _-]?limit|\b(429|529)\b',
  caseSensitive: false,
);
final _keyText = RegExp(
  r'authentication_error|api key was refused|invalid x-api-key|\(HTTP 401\)',
  caseSensitive: false,
);
