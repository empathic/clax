// The empty and error states rendered in every form and theme into
// CLAX_SHOTS_DIR (skipped without it): the start error (closed, and with its
// Details open), an app loading (its mark showing) and failed (with
// Developer tools on), the launcher empty and failed, and the empty inbox
// with its haiku.
import 'dart:convert';
import 'dart:io';
import 'dart:math';

import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/ui/app_cover.dart';
import 'package:clax_app/ui/app_icon.dart';
import 'package:clax_app/ui/error_screen.dart';
import 'package:clax_app/ui/inbox_screen.dart';
import 'package:clax_app/ui/launcher.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../support/fake_voice.dart';
import '../../support/paths.dart';
import 'rig.dart';

const _tail = [
  '2026-10-19T08:14:02.118Z INFO clax_server: listening on 127.0.0.1:7480',
  '2026-10-19T08:14:02.204Z ERROR clax_core::store: database is locked '
      '(code 5) while opening /Users/you/Library/Application Support/Clax/'
      'clax.db',
  'clax exiting with code 1',
];

/// An app called Run tracker with its icon, read from a fake service.
Future<(FakeClaxApi, AppIcons, String)> _runs(Shot s) async {
  final api = FakeClaxApi(clock: () => '2026-10-19T08:00:00Z');
  final aid = api.seedArtifact('Run tracker', {
    'index.html': '<p>runs</p>',
    'icon.svg': File(appPath('test/fixtures/icons/runs.svg')).readAsStringSync(),
  });
  final icons = AppIcons(api);
  final app = (await api.getArtifact(aid)).artifact;
  await s.tester.runAsync(() => icons.want(aid, app.currentVersion));
  return (api, icons, aid);
}

void main() {
  for (final open in [false, true]) {
    shots(open ? 'start-error-details' : 'start-error', (s) async {
      await s.tester.pumpWidget(
        s.app(
          ErrorScreen(
            reason: 'clax stopped 5 times within a minute; last: exited '
                'with code 1',
            tail: _tail,
            onRestart: () {},
          ),
        ),
      );
      if (open) {
        await s.tester.tap(find.text('Details'));
      }
      await s.settle();
      await s.take(open ? 'start-error-details' : 'start-error');
    });
  }

  shots('app-loading', (s) async {
    final (api, icons, aid) = await _runs(s);
    final app = (await api.getArtifact(aid)).artifact;
    await s.tester.pumpWidget(
      s.app(
        Scaffold(body: AppCover(app: app, icons: icons)),
        title: app.title,
      ),
    );
    // Past the 400 ms the mark waits, partway into its breath.
    await s.settle(9);
    await s.take('app-loading');
    await s.tester.pumpWidget(const SizedBox());
  });

  shots('app-failed', (s) async {
    final (api, icons, aid) = await _runs(s);
    final app = (await api.getArtifact(aid)).artifact;
    await s.tester.pumpWidget(
      s.app(
        Scaffold(
          body: AppCover(
            app: app,
            icons: icons,
            failure: 'Clax’s service didn’t answer.',
            detail: 'net::ERR_CONNECTION_REFUSED',
            onTryAgain: () {},
            onBack: () {},
          ),
        ),
        title: app.title,
      ),
    );
    await s.settle();
    await s.take('app-failed');
  });

  Widget launcher(FakeClaxApi api) => Launcher(
    api: api,
    open: (_) {},
    make: (r, cancelled, stage) => throw StateError('unused'),
    voice: FakeVoice.unsupported('No speech here'),
    icons: AppIcons(api),
  );

  shots('launcher-empty', (s) async {
    await s.tester.pumpWidget(s.app(launcher(FakeClaxApi())));
    await s.settle();
    await s.take('launcher-empty');
  });

  shots('launcher-error', (s) async {
    final api = FakeClaxApi()..failNext('listArtifacts', StateError('down'));
    await s.tester.pumpWidget(s.app(launcher(api)));
    await s.settle();
    await s.take('launcher-error');
  });

  shots('inbox-empty', (s) async {
    final api = FakeClaxApi();
    await s.tester.pumpWidget(
      s.app(
        InboxScreen(
          api: api,
          onOpen: (_) {},
          // Read here: an asset read need not finish in a render's fake time.
          haikus: () async => [
            for (final h in jsonDecode(
              File(appPath('assets/haiku.json')).readAsStringSync(),
            ) as List)
              h as String,
          ],
          random: Random(4),
        ),
        title: 'Inbox',
      ),
    );
    await s.settle();
    await s.take('inbox-empty');
  });
}
