// The live build eval's grants (tool/eval/build_eval.dart, HttpGrants)
// against the real debug `clax` named by `CLAX_TEST_BIN`: the published app
// gets what its consent prompts would give it, before its screenshots.
import 'dart:io';

import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/http_api.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../tool/eval/build_eval.dart';
import '../support/real_daemon.dart';
import '../support/paths.dart';

void main() {
  group('grants', () {
    final bin = claxTestBin();
    test(
      'the published app gets its declared net hosts and AI from the '
      'daemon',
      () async {
        final d = await RealDaemon.start(bin!);
        final api = HttpClaxApi(() => d.endpoint);
        addTearDown(api.close);
        await api.registerAppSession(installId: 'eval', pid: pid);
        final made = await api.createArtifact(
          PublishRequest(
            title: 'Split',
            capabilities: {
              'net': {
                'hosts': ['api.frankfurter.app'],
              },
              'sample': <String, Object?>{},
            },
            files: {'index.html': '<!doctype html><title>Split</title>'},
          ),
        );
        final aid = made.artifact.id;
        final granted = await HttpGrants(() => d.endpoint)
            .grant(aid, made.artifact.capabilities);
        expect(granted, ['net (api.frankfurter.app)', 'sample']);
        final net = await api.netGrants(aid);
        expect([for (final g in net.granted) g.host], ['api.frankfurter.app']);
        expect((await api.sampleGrant(aid)).granted, isTrue);
      },
      skip: bin == null ? 'CLAX_TEST_BIN is not set' : false,
      timeout: const Timeout(Duration(minutes: 3)),
    );
  });
}
