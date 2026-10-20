// The live build eval's grants (tool/eval/build_eval.dart, ApiGrants)
// against the real embedded core (the library `CLAX_EMBED_LIB` names): the
// published app gets what its consent prompts would give it, before its
// screenshots.
import 'dart:io';

import 'package:clax_app/clax/api.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../tool/eval/build_eval.dart';
import '../support/real_core.dart';

void main() {
  group('grants', () {
    test(
      'the published app gets its declared net hosts and AI from the core',
      () async {
        final core = await RealCore.open();
        final api = core.api();
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
        final granted = await ApiGrants(api.transport)
            .grant(aid, made.artifact.capabilities);
        expect(granted, ['net (api.frankfurter.app)', 'sample']);
        final net = await api.netGrants(aid);
        expect([for (final g in net.granted) g.host], ['api.frankfurter.app']);
        expect((await api.sampleGrant(aid)).granted, isTrue);
      },
      skip: RealCore.skip,
      timeout: const Timeout(Duration(minutes: 3)),
    );
  });
}
