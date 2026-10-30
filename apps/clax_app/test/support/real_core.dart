import 'dart:async';
import 'dart:ffi';
import 'dart:io';

import 'package:clax_app/clax/http_api.dart';
import 'package:clax_app/core/core_client.dart';
import 'package:clax_app/core/core_host.dart';
import 'package:clax_app/core/core_transport.dart';
import 'package:clax_app/core/ffi.dart';
import 'package:clax_app/core/origins.dart';
import 'package:flutter_test/flutter_test.dart';

import 'paths.dart';

/// The embedded core of crates/clax-embed as the host builds it (the
/// library `CLAX_EMBED_LIB` names), opened by the app's [CoreHost] on a
/// scratch home under [scratchRoot], and closed, then its directory
/// deleted, in tear-downs that run on a failure or a timeout too.
class RealCore {
  RealCore._(this.dir, this.host);

  /// Why a test of the real core cannot run here, or null when it can.
  static Object get skip =>
      claxEmbedLib() == null ? 'CLAX_EMBED_LIB is not set' : false;

  /// The origins the core is opened with: the Apple platforms' scheme.
  static final origins = AppOrigins.forPlatform(android: false);

  /// Holds `support/` (the host's support directory) and its `clax-home/`.
  final Directory dir;
  final CoreHost host;

  /// The shell's origin, the base of the pages a run renders.
  Uri get shell => Uri.parse(origins.shell);

  /// Opens the core and waits until it is ready; throws when it fails.
  static Future<RealCore> open() async {
    final dir = scratchRoot().createTempSync('core-real-');
    addTearDown(() {
      if (dir.existsSync()) dir.deleteSync(recursive: true);
    });
    final support = Directory('${dir.path}/support')..createSync();
    final host = CoreHost(
      home: '${support.path}/clax-home',
      supportDir: support.path,
      origins: origins,
      apiKey: () async => null,
      lib: CoreLib(DynamicLibrary.open(claxEmbedLib()!)),
    );
    // Run before the directory's delete, as tear-downs run last first.
    addTearDown(host.close);
    host.open();
    final settled = Completer<CoreOpen>();
    void check() {
      final s = host.state.value;
      if ((s is CoreReady || s is CoreFailed || s is CoreClosed) &&
          !settled.isCompleted) {
        settled.complete(s);
      }
    }

    host.state.addListener(check);
    check();
    final s = await settled.future.timeout(const Duration(minutes: 1));
    host.state.removeListener(check);
    if (s is! CoreReady) throw StateError('the core did not open: $s');
    return RealCore._(dir, host);
  }

  /// [HttpClaxApi] over the core, as the app makes it ([CoreTransport]),
  /// closed in a tear-down.
  HttpClaxApi api({Future<void> Function(Duration)? eventRetryDelay}) {
    final api = HttpClaxApi.over(
      CoreTransport(host.calls, retryDelay: eventRetryDelay),
      eventRetryDelay: eventRetryDelay,
    );
    addTearDown(api.close);
    return api;
  }
}
