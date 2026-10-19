// The inbox as a list and the item beside it, rendered into CLAX_SHOTS_DIR
// (skipped without it), light and dark: the Mac's Inbox window (900×640,
// under a stand-in for its toolbar with All, Unread and Questions, Mark all
// read and the search field, which flutter_test cannot draw) and Android
// wide, with nothing selected (`inbox-split-empty`), a reply's thread
// selected (`inbox-split`) and a new version selected
// (`inbox-split-version`).
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/ui/developer.dart';
import 'package:clax_app/ui/inbox/inbox_split.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/tokens.g.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../support/fake_voice.dart';
import 'rig.dart';

const _page = {'index.html': '<h1>Runs</h1>'};

/// The Mac's Inbox window: 900×640 at first.
const _window = ShotForm('mac', Size(900, 640), 2, TargetPlatform.macOS);

/// The Inbox window's unified toolbar, as AppKit draws it: the window
/// buttons and title, then All, Unread and Questions as one segmented
/// control, Mark all read and the search field at the trailing end.
class _InboxBand extends StatelessWidget {
  const _InboxBand();

  @override
  Widget build(BuildContext context) {
    final dark = Theme.of(context).brightness == Brightness.dark;
    final p = dark ? ClaxTokens.dark : ClaxTokens.light;
    final body = ClaxType.of(ClaxRole.body, TargetPlatform.macOS);
    Widget light(Color c) => Container(
      width: 12,
      height: 12,
      margin: const EdgeInsets.only(right: 8),
      decoration: BoxDecoration(color: c, shape: BoxShape.circle),
    );
    Widget segment(String label, {bool on = false}) => Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 3),
      decoration: BoxDecoration(
        color: on ? p.surfaceContainers[0] : null,
        borderRadius: BorderRadius.circular(5),
        boxShadow: on
            ? [BoxShadow(color: Colors.black.withValues(alpha: 0.12), blurRadius: 1)]
            : null,
      ),
      child: Text(label, style: body.copyWith(color: p.fg)),
    );
    return Material(
      color: p.card,
      child: Container(
        height: 52,
        padding: const EdgeInsets.only(left: 20, right: 12),
        decoration: BoxDecoration(
          border: Border(bottom: BorderSide(color: p.border)),
        ),
        child: Row(
          children: [
            light(const Color(0xFFFF5F57)),
            light(const Color(0xFFFEBC2E)),
            light(const Color(0xFF28C840)),
            const SizedBox(width: 20),
            Text(
              'Inbox',
              style: ClaxType.of(
                ClaxRole.headline,
                TargetPlatform.macOS,
              ).copyWith(color: p.fg),
            ),
            const Spacer(),
            Container(
              padding: const EdgeInsets.all(2),
              decoration: BoxDecoration(
                color: p.surfaceContainers[3],
                borderRadius: BorderRadius.circular(7),
              ),
              child: Row(
                children: [
                  segment('All', on: true),
                  segment('Unread'),
                  segment('Questions'),
                ],
              ),
            ),
            const SizedBox(width: 12),
            Icon(Icons.check_circle_outline_rounded, size: 18, color: p.muted),
            const SizedBox(width: 12),
            Container(
              width: 180,
              height: 26,
              padding: const EdgeInsets.symmetric(horizontal: 8),
              decoration: BoxDecoration(
                color: p.surfaceContainers[3],
                borderRadius: BorderRadius.circular(7),
              ),
              child: Row(
                children: [
                  Icon(Icons.search_rounded, size: 15, color: p.muted),
                  const SizedBox(width: 4),
                  Text('Search the inbox', style: body.copyWith(color: p.muted)),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

Future<void> _split(Shot s, {String? select}) async {
  final api = FakeClaxApi(clock: () => '2026-10-12T08:00:00Z');
  final runs = api.seedArtifact('Runs', _page);
  final budget = api.seedArtifact('Household budget', _page);
  final weather = api.seedArtifact('Weather on the walk to work', _page);
  await api.registerAppSession(installId: 'i', pid: 1);
  final asked = api.sendComment(
    runs,
    'Show distances in km',
    anchor: const {
      'kind': 'element',
      'file': 'index.html',
      'name': 'Table under ‘This week’',
    },
  );
  await api.replyAsAgent(
    runs,
    asked.threadId,
    'Done. Distances now read in km, and the weekly total sits under the '
    'list.',
  );
  api.pushInbox('version', artifactId: budget, versionN: 4, note: 'Totals');
  api.pushInbox(
    'reply',
    artifactId: runs,
    threadId: asked.threadId,
    body: 'Done. Distances now read in km, and the weekly total sits under '
        'the list.',
  );
  api.pushInbox('published', artifactId: weather);
  final old = api.pushInbox(
    'finished',
    artifactId: budget,
    message: 'All three fixed',
  );
  await api.markInboxRead(old.id);
  final mac = s.form.platform == TargetPlatform.macOS;
  final dev = ValueNotifier(false);
  addTearDown(dev.dispose);
  final split = InboxSplit(
    api: api,
    stream: api.stream(),
    voice: FakeVoice(),
    toolbar: mac,
    now: () => DateTime.utc(2026, 10, 12, 9, 30),
    onOpenApp: (aid, {thread, version, comment}) {},
  );
  await s.tester.pumpWidget(
    DeveloperScope(
      notifier: dev,
      child: mac
          ? MaterialApp(
              debugShowCheckedModeBanner: false,
              theme: claxTheme(Brightness.light, TargetPlatform.macOS),
              darkTheme: claxTheme(Brightness.dark, TargetPlatform.macOS),
              themeMode: s.dark ? ThemeMode.dark : ThemeMode.light,
              home: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  const _InboxBand(),
                  Expanded(child: split),
                ],
              ),
            )
          : s.app(split, title: 'Inbox'),
    ),
  );
  await s.settle();
  if (select != null) {
    await s.tester.tap(find.textContaining(select).first);
    await s.settle();
  }
}

void main() {
  for (final forms in [
    [_window],
    [wide],
  ]) {
    shots('inbox-split-empty', (s) async {
      await _split(s);
      await s.take('inbox-split-empty');
    }, forms: forms);
    shots('inbox-split', (s) async {
      await _split(s, select: 'Clax replied');
      await s.take('inbox-split');
    }, forms: forms);
    shots('inbox-split-version', (s) async {
      await _split(s, select: 'Version 4 is ready');
      await s.take('inbox-split-version');
    }, forms: forms);
  }
}
