import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:io';

import 'package:clax_app/core/core_client.dart';
import 'package:clax_app/core/ffi.dart';
import 'package:clax_app/core/frame.dart';
import 'package:clax_app/core/origins.dart';
import 'package:ffi/ffi.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/paths.dart';

typedef _OpenC =
    Uint64 Function(Pointer<Uint8>, Size, Pointer<Void>, Int64);
typedef _OpenD = int Function(Pointer<Uint8>, int, Pointer<Void>, int);
typedef _CallC = Int32 Function(Uint64, Pointer<Uint8>, Size, Int64, Int64);
typedef _CallD = int Function(int, Pointer<Uint8>, int, int, int);
typedef _OneC = Int32 Function(Uint64);
typedef _OneD = int Function(int);
typedef _SubC = Int32 Function(Uint64, Int64);
typedef _SubD = int Function(int, int);

/// The core of crates/clax-embed, as the host builds it
/// (//crates/clax-embed:clax_embed_host), through its four C functions.
void main() {
  final path = claxEmbedLib();
  final skip = path == null ? 'CLAX_EMBED_LIB is not set' : false;

  test('the C functions refuse NULL pointers and unknown handles', () {
    final dl = DynamicLibrary.open(path!);
    final open = dl.lookupFunction<_OpenC, _OpenD>('clax_embed_open');
    final call = dl.lookupFunction<_CallC, _CallD>('clax_embed_call');
    final subscribe = dl.lookupFunction<_SubC, _SubD>('clax_embed_subscribe');
    final close = dl.lookupFunction<_OneC, _OneD>('clax_embed_close');
    final post = NativeApi.postCObject.cast<Void>();
    expect(open(nullptr, 4, post, 0), 0, reason: 'a NULL home');
    // A NULL post with a real home: 0 before the home is read.
    final home = ascii.encode('/nonexistent');
    final buf = malloc<Uint8>(home.length);
    addTearDown(() => malloc.free(buf));
    buf.asTypedList(home.length).setAll(0, home);
    expect(open(buf, home.length, nullptr, 0), 0, reason: 'a NULL post');
    expect(call(1 << 40, nullptr, 8, 0, 0), 2, reason: 'a NULL request');
    expect(call(1 << 40, buf, home.length, 0, 0), 1, reason: 'an unknown handle');
    expect(subscribe(1 << 40, 0), 1);
    expect(close(1 << 40), 1);
  }, skip: skip);

  test('open, call, subscribe and close a core on a new home', () async {
    final home = scratchRoot().createTempSync('core-real-');
    addTearDown(() => home.deleteSync(recursive: true));
    final core = CoreClient.open(
      home.path,
      AppOrigins.forPlatform(android: false),
      lib: CoreLib(DynamicLibrary.open(path!)),
    );
    expect(core.handle, isNot(0));
    final states = <CoreOpen>[];
    final ready = Completer<void>();
    final closed = Completer<void>();
    core.states.listen((s) {
      states.add(s);
      switch (s) {
        case CoreReady():
          ready.complete();
        case CoreFailed():
          ready.completeError(StateError('the core failed: $s'));
        case CoreClosed():
          closed.complete();
        case CoreOpening():
      }
    });

    // A call made before `ready` waits for it.
    final early = core.call(const CoreRequest('GET', '/api/artifacts'));
    await ready.future.timeout(const Duration(minutes: 1));
    final migrating = states.whereType<CoreOpening>().where(
      (s) => s.phase == 'migrating',
    );
    expect(migrating, isNotEmpty, reason: '$states');
    for (final m in migrating) {
      expect(m.done! <= m.total!, isTrue, reason: '$m');
    }
    expect(states.last, const CoreReady());

    final r = switch (await early) {
      CoreAnswered(:final response) => response,
      final u => throw TestFailure('$u'),
    };
    expect(r.status, 200, reason: utf8.decode(r.body));
    expect(r.header('content-type'), startsWith('application/json'));
    expect(jsonDecode(utf8.decode(r.body)), {'artifacts': isEmpty});

    // The core, not the caller, decides what a frame may reach.
    final refused = switch (await core.call(
      const CoreRequest('GET', '/api/artifacts', as: CallAs.shellPage),
    )) {
      CoreAnswered(:final response) => response,
      final u => throw TestFailure('$u'),
    };
    expect(refused.status, 404);
    expect(utf8.decode(refused.body), contains('not_in_app'));

    final sub = core.subscribe();
    final blocks = <String>[];
    final first = Completer<void>();
    final ended = Completer<void>();
    sub.blocks.listen((b) {
      blocks.add(b);
      if (!first.isCompleted) first.complete();
    }, onDone: ended.complete);
    await first.future;
    expect(blocks.first, contains('event: ready'));

    core.close();
    await ended.future;
    expect(blocks.last, endBlock('core_closed'));
    await closed.future;
    expect(states.last, const CoreClosed());
    final after = await core.call(const CoreRequest('GET', '/api/artifacts'));
    expect(after, isA<CoreUnanswered>());
    expect((after as CoreUnanswered).code, 'core_closed');
    expect(File('${home.path}/embed.json').readAsStringSync(), contains('clax://{id}.c'));
  }, skip: skip);
}
