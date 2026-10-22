import 'package:clax_app/agent/apply.dart';
import 'package:clax_app/agent/edit_result.dart';
import 'package:flutter_test/flutter_test.dart';

EditResult result({
  List<FileWrite> files = const [],
  List<TextEdit> edits = const [],
  List<String> removed = const [],
}) => EditResult(reply: 'Done.', files: files, edits: edits, removed: removed);

Matcher miss(String path, int count) => throwsA(
  isA<EditMiss>()
      .having((e) => e.path, 'path', path)
      .having((e) => e.count, 'count', count),
);

void main() {
  test('writes files, applies edits in order and marks removals', () {
    final out = applyResult(
      {'index.html': '<h1>Old</h1><p>a</p>', 'app.js': 'x()', 'old.css': 'p{}'},
      result(
        files: [const FileWrite(path: 'new.js', content: 'y()')],
        edits: [
          const TextEdit(path: 'index.html', oldStr: 'Old', newStr: 'New'),
          const TextEdit(path: 'index.html', oldStr: 'New', newStr: r'$1 New'),
        ],
        removed: ['old.css'],
      ),
    );
    expect(out, {
      'new.js': 'y()',
      'index.html': r'<h1>$1 New</h1><p>a</p>',
      'old.css': null,
    });
  });

  test('an edit applies to a file written in the same result', () {
    final out = applyResult(
      {},
      result(
        files: [const FileWrite(path: 'index.html', content: 'a b')],
        edits: [const TextEdit(path: 'index.html', oldStr: 'b', newStr: 'c')],
      ),
    );
    expect(out, {'index.html': 'a c'});
  });

  test('a missing old_str is a miss with count 0', () {
    expect(
      () => applyResult(
        {'index.html': 'abc'},
        result(
          edits: [const TextEdit(path: 'index.html', oldStr: 'z', newStr: '')],
        ),
      ),
      miss('index.html', 0),
    );
  });

  test('an ambiguous old_str is a miss with its count, overlaps included', () {
    expect(
      () => applyResult(
        {'index.html': '<li>x</li><li>x</li>'},
        result(
          edits: [
            const TextEdit(path: 'index.html', oldStr: '<li>x', newStr: ''),
          ],
        ),
      ),
      miss('index.html', 2),
    );
    expect(
      () => applyResult(
        {'a.txt': 'aaa'},
        result(
          edits: [const TextEdit(path: 'a.txt', oldStr: 'aa', newStr: '')],
        ),
      ),
      miss('a.txt', 2),
    );
  });

  test('an edit on a file without text is a miss', () {
    expect(
      () => applyResult(
        {},
        result(
          edits: [const TextEdit(path: 'logo.png', oldStr: 'a', newStr: '')],
        ),
      ),
      miss('logo.png', 0),
    );
  });

  test('the miss reason names the path and what was wrong', () {
    expect(
      const EditMiss('index.html', 'Save', 3).reason,
      allOf(contains('"index.html"'), contains('"Save"'), contains('3 times')),
    );
    expect(
      const EditMiss('index.html', 'Save', 0).reason,
      contains('not found'),
    );
  });
}
