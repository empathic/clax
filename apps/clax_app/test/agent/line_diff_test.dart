import 'package:clax_app/agent/failure.dart';
import 'package:clax_app/agent/line_diff.dart';
import 'package:clax_app/agent/unfinished.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('lineChanges', () {
    test('counts the lines added and removed over the common ones', () {
      expect(lineChanges('a\nb\nc\n', 'a\nx\nc\nd\n'), (added: 2, removed: 1));
      expect(lineChanges('a\nb\n', 'a\nb\n'), (added: 0, removed: 0));
      expect(lineChanges('b\nc\n', 'a\nb\nc\n'), (added: 1, removed: 0));
    });

    test('a missing side is no file', () {
      expect(lineChanges(null, 'a\nb\n'), (added: 2, removed: 0));
      expect(lineChanges('a\nb\nc', null), (added: 0, removed: 3));
    });

    test('finds moved lines without counting the shared ones', () {
      expect(lineChanges('1\n2\n3\n4\n5\n', '1\n3\n2\n4\n5\n'), (
        added: 1,
        removed: 1,
      ));
    });
  });

  test('changedFiles lists each edited path with its counts, by path', () {
    expect(
      changedFiles(
        {'b.js': null, 'a.html': 'x\ny\n', 'c.css': 'new\n'},
        {'a.html': 'x\n', 'b.js': '1\n2\n'},
      ),
      [
        (path: 'a.html', added: 1, removed: 0),
        (path: 'b.js', added: 0, removed: 2),
        (path: 'c.css', added: 1, removed: 0),
      ],
    );
  });

  group('limitProgressText', () {
    test('names at most five files, then how many more', () {
      final text = limitProgressText(
        LimitProgress(
          stops: 3,
          spentUsd: 2.1,
          changed: [
            for (var i = 1; i <= 7; i++)
              (path: 'f$i.js', added: i, removed: i == 1 ? 0 : 1),
          ],
        ),
      );
      expect(
        text,
        '\n\nStopped 3 times · \$2.10 of \$6 used\n'
        'Changed so far: f1.js (+1), f2.js (+2 −1), f3.js (+3 −1), '
        'f4.js (+4 −1), f5.js (+5 −1), and 2 more',
      );
    });

    test('quotes the latest three notes, a repeated one once', () {
      expect(
        limitProgressText(
          const LimitProgress(
            stops: 1,
            spentUsd: 0.4,
            notes: ['Reading', 'Looking', 'Looking', ' ', 'Fixing', 'Testing'],
          ),
        ),
        '\n\nStopped once · \$0.40 of \$6 used\n\n'
        "The agent's latest notes:\n- Looking\n- Fixing\n- Testing",
      );
    });

    test('a file whose lines all stayed, as a line ending, is named alone', () {
      expect(
        limitProgressText(
          const LimitProgress(
            stops: 2,
            spentUsd: 1,
            changed: [(path: 'a.js', added: 0, removed: 0)],
          ),
        ),
        '\n\nStopped twice · \$1.00 of \$6 used\nChanged so far: a.js',
      );
    });
  });
}
