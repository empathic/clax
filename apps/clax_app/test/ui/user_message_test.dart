import 'dart:async';
import 'dart:io';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/ui/app_surface.dart';
import 'package:clax_app/ui/user_message.dart';
import 'package:flutter_test/flutter_test.dart';

const _noAnswer = 'Clax’s service didn’t answer.';
const _tooLong = 'That took too long. Try again.';
const _busy = 'Claude is busy right now. Try again in a minute.';
const _key = 'Your Claude API key wasn’t accepted. Check it in Settings.';
const _unknown = 'Something went wrong. Try again.';

void main() {
  test('a socket that failed reads as the service not answering', () {
    expect(
      userMessage(const SocketException('Connection refused')),
      _noAnswer,
    );
  });

  test('an HTTP client failure reads as the service not answering', () {
    expect(
      userMessage(const HttpException('Connection closed')),
      _noAnswer,
    );
  });

  test('a page that failed with net::ERR_ reads as the service not '
      'answering', () {
    expect(
      userMessage(
        const SurfaceFault('net::ERR_CONNECTION_REFUSED', reloading: false),
      ),
      _noAnswer,
    );
    expect(userMessage('net::ERR_CONNECTION_REFUSED'), _noAnswer);
  });

  test('a timeout says it took too long', () {
    expect(
      userMessage(TimeoutException('no answer', const Duration(seconds: 5))),
      _tooLong,
    );
  });

  test('an overloaded or rate-limited API says Claude is busy', () {
    expect(
      userMessage(StateError('The Claude API said: overloaded (529)')),
      _busy,
    );
    expect(
      userMessage(const AgentError('Claude API error (HTTP 529): Overloaded')),
      _busy,
    );
    expect(
      userMessage(
        const AgentError(
          'Claude API error mid-answer (rate_limit_error): slow down',
        ),
      ),
      _busy,
    );
    expect(
      userMessage(
        const ClaxApiError(
          status: 529,
          code: 'overloaded',
          message: 'Overloaded',
        ),
      ),
      _busy,
    );
    expect(
      userMessage(
        const ClaxApiError(
          status: 429,
          code: 'rate_limited',
          message: 'Too many requests',
        ),
      ),
      _busy,
    );
  });

  test('the service’s own question limit is not Claude being busy', () {
    expect(
      userMessage(
        const ClaxApiError(
          status: 429,
          code: 'limit_reached',
          message: 'too many open questions',
        ),
      ),
      isNot(_busy),
    );
  });

  test('a refused key asks to check it in Settings', () {
    expect(userMessage(const AgentError('The API key was refused')), _key);
    expect(
      userMessage(
        const ClaxApiError(
          status: 401,
          code: 'authentication_error',
          message: 'invalid x-api-key',
        ),
      ),
      _key,
    );
    expect(
      userMessage(StateError('authentication_error: invalid x-api-key')),
      _key,
    );
  });

  test('another AgentError reads as its own message', () {
    expect(
      userMessage(const AgentError('Add your Claude API key in Settings')),
      'Add your Claude API key in Settings',
    );
  });

  test('anything else reads as something going wrong', () {
    expect(userMessage(StateError('no element')), _unknown);
    expect(userMessage(const FormatException('Unexpected <')), _unknown);
    expect(userMessage(Exception('boom')), _unknown);
  });

  test('no line names a type, a code or Bad state', () {
    for (final e in <Object>[
      StateError('x'),
      const SocketException('refused'),
      const SurfaceFault('net::ERR_FAILED', reloading: false),
      const ClaxApiError(status: 500, code: 'internal', message: 'boom'),
    ]) {
      final line = userMessage(e);
      expect(line, isNot(contains('Bad state')));
      expect(line, isNot(contains('net::')));
      expect(line, isNot(contains('Exception')));
      expect(line, isNot(contains('Error')));
    }
  });

  test('rawError keeps the raw text for the log', () {
    expect(rawError(StateError('no element')), 'Bad state: no element');
    expect(
      rawError(const SurfaceFault('net::ERR_FAILED', reloading: false)),
      'net::ERR_FAILED',
    );
  });
}
