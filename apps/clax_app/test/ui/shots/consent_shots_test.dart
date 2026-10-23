// The consent dialogs rendered in every form and theme into CLAX_SHOTS_DIR
// (skipped without it): Clax asking to read another app, an app asking
// for the device's location, on phones Clax asking before it sends
// anything to the Claude API, and Clax asking before Claude first reads an
// app someone else wrote, and the person deciding whether Clax may read
// comments a stranger wrote.
import 'package:clax_app/agent/read_consent.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/platform/device/device_consent_dialog.dart';
import 'package:clax_app/platform/device/device_host.dart';
import 'package:clax_app/ui/ai_consent_sheet.dart';
import 'package:clax_app/ui/approval_sheet.dart';
import 'package:clax_app/ui/author_trust_sheet.dart';
import 'package:clax_app/ui/read_consent_dialog.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'rig.dart';

/// Shows a page, runs [ask] with its navigator, then [then], and takes
/// the shot.
Future<void> _over(
  Shot s,
  String surface,
  void Function(NavigatorState nav) ask, {
  Future<void> Function()? then,
}) async {
  final nav = GlobalKey<NavigatorState>();
  await s.tester.pumpWidget(
    s.app(
      Navigator(
        key: nav,
        onGenerateRoute: (_) => MaterialPageRoute<void>(
          builder: (_) => Scaffold(
            appBar: AppBar(title: const Text('Dashboard')),
            body: const Center(child: Text('A page')),
          ),
        ),
      ),
    ),
  );
  ask(nav.currentState!);
  await s.settle();
  if (then != null) {
    await then();
    await s.settle();
  }
  await s.take(surface);
}

void main() {
  void read(NavigatorState nav) {
    Future<ConsentAnswer> _ =
        DialogReadConsent(
          navigator: () => nav,
          memory: MemoryConsentMemory(),
        ).ask(
          reader: 'a0000000000a',
          readerTitle: 'Dashboard',
          source: 'b0000000000b',
          sourceTitle: 'Groceries',
        );
  }

  // The read consent is a ClaxDialog without content: the Apple family is
  // drawn on the iPhone, as the Mac shows it as an NSAlert sheet
  // (NativeDialogs.alert), which flutter_test cannot draw.
  const readForms = [phone, wide, iphone];
  shots(
    'consent-read',
    (s) => _over(s, 'consent-read', read),
    forms: readForms,
  );
  // "Always for this app" ticked: the button reads Always allow.
  shots(
    'consent-read-always',
    (s) => _over(
      s,
      'consent-read-always',
      read,
      then: () => s.tester.tap(find.text('Always for this app')),
    ),
    forms: readForms,
  );
  shots(
    'consent-location',
    (s) => _over(s, 'consent-location', (nav) {
      Future<DeviceAnswer> _ = DialogDeviceConsent(
        navigator: () => nav,
        title: (_) async => 'Plant care',
      ).ask('abcdefghjkmn', 'location');
    }),
  );
  shots(
    'consent-ai',
    (s) => _over(s, 'consent-ai', (nav) {
      Future<bool?> _ = showAiConsentSheet(nav.context);
    }),
    forms: const [iphone, phone],
  );
  shots(
    'consent-author',
    (s) => _over(s, 'consent-author', (nav) {
      Future<bool?> _ = showAuthorTrustSheet(
        nav.context,
        app: 'Plant log',
        who: 'Sam',
      );
    }),
  );
  shots(
    'consent-approval',
    (s) => _over(s, 'consent-approval', (nav) {
      Future<ApprovalRequest?> _ = showApprovalSheet(
        nav.context,
        api: FakeClaxApi(),
        app: 'Plant log',
        r: const ApprovalRequest(
          id: 'r1',
          author: CardPerson(
            publicId: 'remote:clax.dev#u_bob',
            name: 'Bob (on clax.dev)',
          ),
          artifactId: 'a0000000000a',
          threadId: 't1',
          count: 2,
          status: 'open',
          createdAt: '2026-10-22T09:00:00Z',
          comments: [
            HeldComment(
              id: 'c1',
              body: 'Water the fern every other day, not daily.',
              createdAt: '2026-10-22T09:00:00Z',
            ),
            HeldComment(
              id: 'c2',
              body: 'And add a column for the last repotting.',
              createdAt: '2026-10-22T09:01:00Z',
            ),
          ],
        ),
      );
    }),
  );
}
