import 'dart:async';
import 'dart:io';

import 'package:clax_app/agent/author_trust.dart';
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/queue/client.dart' show Cancel, Change;
import 'package:clax_app/queue/relay.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_queue.dart';

const _sam = 'SSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSS';

Change _change(String id, String aid) =>
    Change(id, aid, base: 1, threads: const {'t1': 'c1'});

void main() {
  test('a change goes to the queue only once admitted; what is sent while '
      'one is asked about goes after it, in order', () async {
    final queue = FakeQueueClient();
    final asked = <String, Completer<bool>>{};
    final relay = QueueRelay(
      admit: (c) => (asked[c.artifactId] = Completer<bool>()).future,
    )..attach(queue);

    relay.send(_change('j1', 'a_sam'));
    relay.send(const Cancel('j0'));
    await pumpEventQueue();
    expect(queue.sent, isEmpty, reason: 'waits for the answer');

    asked['a_sam']!.complete(false);
    await pumpEventQueue();
    expect(queue.sent, [const Cancel('j0')], reason: 'the change is dropped');

    relay.send(_change('j2', 'a_mine'));
    await pumpEventQueue();
    asked['a_mine']!.complete(true);
    await pumpEventQueue();
    expect(queue.sent, [const Cancel('j0'), _change('j2', 'a_mine')]);

    // With nothing being asked about, other intents go at once.
    relay.send(const Cancel('j2'));
    expect(queue.sent.last, const Cancel('j2'));
  });

  test('an app installed from a stranger makes no change job until the '
      'person trusts its author', () async {
    final dir = Directory.systemTemp.createTempSync('clax-relay-');
    addTearDown(() => dir.deleteSync(recursive: true));
    final lineages = LineageStore(dir);
    await lineages.put(
      'a_sam',
      LineageEntry(
        lineage: 'l_aaaaaaaaaaaaaaaaaaaaaaaaaa',
        role: LineageRole.installed,
        key: _sam,
        name: 'Sam',
        version: 1,
        released: const [],
        capabilities: const {},
        title: 'Plant log',
        fromBundle: true,
      ),
    );
    final trust = AuthorTrust(
      lineages: lineages,
      settings: SettingsStore(File('${dir.path}/settings.json')),
    );
    final queue = FakeQueueClient();
    // No screen to ask on: the author is not trusted.
    final relay = QueueRelay(admit: (c) => trust.ensure(null, c.artifactId))
      ..attach(queue);
    relay
      ..send(_change('j1', 'a_sam'))
      ..send(_change('j2', 'a_made_here'));
    await pumpEventQueue();
    expect(queue.sentOf<Change>().map((c) => c.artifactId), ['a_made_here']);
  });
}
