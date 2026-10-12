import 'dart:io';

import 'package:clax_app/agent/edit_result.dart';
import 'package:clax_app/agent/ledger.dart';
import 'package:flutter_test/flutter_test.dart';

ChangeRecord record(String aid, int base) => ChangeRecord(
  artifactId: aid,
  base: base,
  addresses: ['t1'],
  answered: {
    't1': ['c1'],
    't2': ['c2'],
  },
  reply: 'Done.',
  resolve: true,
  db: const [
    DbOp(op: 'set', collection: 'runs', docId: 'a', data: {'km': 5}),
    DbOp(op: 'delete', collection: 'runs', docId: 'b'),
  ],
  pins: {'runs/a': 3, 'runs/b': null},
);

void main() {
  test(
    'a memory ledger replaces by artifact and base and keeps the newest',
    () async {
      final l = MemoryChangeLedger(limit: 2);
      await l.put(record('a', 1));
      await l.put(record('a', 1)..docsDone = true);
      expect((await l.of('a')).single.docsDone, isTrue);
      await l.put(record('a', 2));
      await l.put(record('b', 1));
      expect([for (final r in await l.of('a')) r.base], [2]);
      await l.remove('a', 2);
      expect(await l.of('a'), isEmpty);
    },
  );

  test(
    'a file ledger survives a new instance, record fields included',
    () async {
      final dir = await Directory.systemTemp.createTemp('ledger');
      addTearDown(() => dir.delete(recursive: true));
      final file = File('${dir.path}/ledger.json');
      await FileChangeLedger(file).put(record('a', 4)..docsDone = true);
      final r = (await FileChangeLedger(file).of('a')).single;
      expect(r.base, 4);
      expect(r.addresses, ['t1']);
      expect(r.answered, {
        't1': ['c1'],
        't2': ['c2'],
      });
      expect(r.reply, 'Done.');
      expect(r.resolve, isTrue);
      expect(
        [for (final o in r.db) '${o.op} ${o.collection}/${o.docId}'],
        ['set runs/a', 'delete runs/b'],
      );
      expect(r.db.first.data, {'km': 5});
      expect(r.pins, {'runs/a': 3, 'runs/b': null});
      expect(r.docsDone, isTrue);
      expect(File('${file.path}.tmp').existsSync(), isFalse);
    },
  );

  test('an unreadable ledger file reads as empty', () async {
    final dir = await Directory.systemTemp.createTemp('ledger');
    addTearDown(() => dir.delete(recursive: true));
    final file = File('${dir.path}/ledger.json')..writeAsStringSync('{oops');
    expect(await FileChangeLedger(file).of('a'), isEmpty);
  });
}
