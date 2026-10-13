import 'dart:convert';
import 'dart:io';

import 'package:clax_app/agent/asked.dart';
import 'package:flutter_test/flutter_test.dart';

AskedEntry entry(String qid, {String aid = 'a1'}) => AskedEntry(
  questionId: qid,
  artifactId: aid,
  threadIds: ['t-$qid'],
  commentIds: ['c-$qid', 'c2-$qid'],
);

void main() {
  group('MemoryAskedQuestions', () {
    test('put, take and drop', () async {
      final m = MemoryAskedQuestions();
      await m.put(entry('q1'));
      await m.put(entry('q2'));
      expect([for (final e in await m.all()) e.questionId], ['q1', 'q2']);
      final taken = await m.take('q1');
      expect(taken?.threadIds, ['t-q1']);
      expect(taken?.commentIds, ['c-q1', 'c2-q1']);
      expect(await m.take('q1'), isNull);
      await m.drop('q2');
      await m.drop('unknown');
      expect(await m.all(), isEmpty);
    });

    test(
      'answer keeps the late-answer text on an entry that is there',
      () async {
        final m = MemoryAskedQuestions();
        await m.put(entry('q1'));
        expect((await m.answer('q1', 'km'))?.answer, 'km');
        expect((await m.all()).single.answer, 'km');
        expect(await m.answer('gone', 'x'), isNull);
        expect(await m.all(), hasLength(1));
      },
    );

    test('a put of the same question replaces it', () async {
      final m = MemoryAskedQuestions();
      await m.put(entry('q1'));
      await m.put(entry('q1', aid: 'a2'));
      final all = await m.all();
      expect(all, hasLength(1));
      expect(all.single.artifactId, 'a2');
    });

    test('keeps at most limit entries, dropping the oldest', () async {
      final m = MemoryAskedQuestions(limit: 2);
      for (final q in ['q1', 'q2', 'q3']) {
        await m.put(entry(q));
      }
      expect([for (final e in await m.all()) e.questionId], ['q2', 'q3']);
    });
  });

  group('FileAskedQuestions', () {
    late Directory dir;
    late File file;

    setUp(() async {
      dir = await Directory.systemTemp.createTemp('asked_test');
      file = File('${dir.path}/support/asked.json');
    });
    tearDown(() => dir.delete(recursive: true));

    test('survives a reload', () async {
      final a = FileAskedQuestions(file);
      await a.put(entry('q1'));
      await a.put(entry('q2'));
      await a.drop('q2');

      final b = FileAskedQuestions(file);
      final all = await b.all();
      expect(all, hasLength(1));
      expect(all.single.questionId, 'q1');
      expect(all.single.artifactId, 'a1');
      expect(all.single.threadIds, ['t-q1']);
      expect(all.single.commentIds, ['c-q1', 'c2-q1']);
    });

    test('a late answer is kept across a reload', () async {
      final a = FileAskedQuestions(file);
      await a.put(entry('q1'));
      await a.put(entry('q2'));
      await a.answer('q1', '[clax] The person answered …');
      final all = await FileAskedQuestions(file).all();
      expect(
        [for (final e in all) e.answer],
        ['[clax] The person answered …', null],
      );
      expect(
        jsonDecode(file.readAsStringSync())['entries'][1],
        isNot(contains('answer')),
      );
    });

    test('a take is kept across a reload', () async {
      final a = FileAskedQuestions(file);
      await a.put(entry('q1'));
      expect((await a.take('q1'))?.questionId, 'q1');
      expect(await FileAskedQuestions(file).all(), isEmpty);
      expect(await FileAskedQuestions(file).take('q1'), isNull);
    });

    test('writes through a temporary file and leaves none behind', () async {
      final a = FileAskedQuestions(file);
      await a.put(entry('q1'));
      expect(await file.exists(), isTrue);
      expect(await File('${file.path}.tmp').exists(), isFalse);
      final j = jsonDecode(await file.readAsString()) as Map;
      expect(j['version'], 1);
      expect((j['entries'] as List).single, {
        'question_id': 'q1',
        'artifact_id': 'a1',
        'thread_ids': ['t-q1'],
        'comment_ids': ['c-q1', 'c2-q1'],
      });
    });

    test('calls made at once are all kept', () async {
      final a = FileAskedQuestions(file);
      await Future.wait([for (var i = 0; i < 10; i++) a.put(entry('q$i'))]);
      await Future.wait([a.drop('q3'), a.take('q4')]);
      final all = await FileAskedQuestions(file).all();
      expect(
        [for (final e in all) e.questionId],
        [
          for (var i = 0; i < 10; i++)
            if (i != 3 && i != 4) 'q$i',
        ],
      );
    });

    test('an unreadable file reads as empty and is replaced', () async {
      await file.parent.create(recursive: true);
      await file.writeAsString('{"entries": [');
      final a = FileAskedQuestions(file);
      expect(await a.all(), isEmpty);
      await a.put(entry('q1'));
      expect((await FileAskedQuestions(file).all()).single.questionId, 'q1');
    });

    test('a malformed entry is skipped, the others kept', () async {
      await file.parent.create(recursive: true);
      await file.writeAsString(
        jsonEncode({
          'version': 1,
          'entries': [
            entry('q1').toJson(),
            {'question_id': 'q2', 'artifact_id': 7},
            'junk',
            entry('q3').toJson(),
          ],
        }),
      );
      final all = await FileAskedQuestions(file).all();
      expect([for (final e in all) e.questionId], ['q1', 'q3']);
    });

    test('a save cut before its rename is recovered from the temporary '
        'file when the file itself is missing', () async {
      await file.parent.create(recursive: true);
      await File('${file.path}.tmp').writeAsString(
        jsonEncode({
          'version': 1,
          'entries': [entry('q1').toJson()],
        }),
      );
      final all = await FileAskedQuestions(file).all();
      expect(all.single.questionId, 'q1');
    });

    test('a whole temporary file beside the file is the newer state and '
        'wins', () async {
      final a = FileAskedQuestions(file);
      await a.put(entry('q1'));
      await File('${file.path}.tmp').writeAsString(
        jsonEncode({
          'version': 1,
          'entries': [entry('q1').toJson(), entry('q2').toJson()],
        }),
      );
      final b = FileAskedQuestions(file);
      expect([for (final e in await b.all()) e.questionId], ['q1', 'q2']);
      await b.drop('q1');
      expect(await File('${file.path}.tmp').exists(), isFalse);
      expect(
        [for (final e in await FileAskedQuestions(file).all()) e.questionId],
        ['q2'],
      );
    });

    test('a cut temporary file is ignored beside a whole file', () async {
      final a = FileAskedQuestions(file);
      await a.put(entry('q1'));
      await File('${file.path}.tmp').writeAsString('{"version": 1, "ent');
      final all = await FileAskedQuestions(file).all();
      expect(all.single.questionId, 'q1');
    });

    test('keeps at most limit entries', () async {
      final a = FileAskedQuestions(file, limit: 2);
      for (final q in ['q1', 'q2', 'q3']) {
        await a.put(entry(q));
      }
      expect(
        [for (final e in await FileAskedQuestions(file).all()) e.questionId],
        ['q2', 'q3'],
      );
    });
  });
}
