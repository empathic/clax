import 'dart:convert';
import 'dart:io';
import 'dart:ui' show Color, Rect;

import 'package:clax_app/ui/host_protocol.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('HostMessage.parse', () {
    test('ready', () {
      final m = HostMessage.parse(
        '{"type":"ready","artifact":"a1","version":3,"latest":4,"file":"index.html"}',
      );
      expect(m, isA<Ready>());
      final r = m as Ready;
      expect(r.artifact, 'a1');
      expect(r.version, 3);
      expect(r.latest, 4);
      expect(r.file, 'index.html');
      expect(r.pinned, isTrue);
    });

    test('chrome: the page\'s top and bottom colours, #rrggbb only', () {
      final m = HostMessage.parse(
        '{"type":"chrome","top":"#a0b0c0","bottom":"#FFFFFF"}',
      );
      expect(m, isA<ChromeMsg>());
      final c = m as ChromeMsg;
      expect(c.top, const Color(0xFFA0B0C0));
      expect(c.bottom, const Color(0xFFFFFFFF));
      for (final bad in <Object?>['red', '#abc', '#a0b0c0ff', 'a0b0c0', 0xa0b0c0, null]) {
        expect(
          HostMessage.fromJson({'type': 'chrome', 'top': bad, 'bottom': '#ffffff'}),
          isNull,
          reason: '$bad',
        );
      }
    });

    test('state with a draft and without one', () {
      final m = HostMessage.parse(
        jsonEncode({
          'type': 'state',
          'commenting': true,
          'draft': {'label': 'h1', 'text': 'bigger', 'clip': 'pending'},
          'busy': false,
        }),
      );
      final s = m as StateMsg;
      expect(s.commenting, isTrue);
      expect(s.busy, isFalse);
      expect(s.draft!.label, 'h1');
      expect(s.draft!.text, 'bigger');
      expect(s.draft!.clip, DraftClip.pending);

      final none = HostMessage.parse(
        '{"type":"state","commenting":false,"draft":null,"busy":true}',
      ) as StateMsg;
      expect(none.draft, isNull);
      expect(none.busy, isTrue);
      expect(
        (HostMessage.parse(
          '{"type":"state","commenting":true,"draft":{"label":"","text":"","clip":"ready"},"busy":false}',
        ) as StateMsg).draft!.clip,
        DraftClip.ready,
      );
      expect(
        (HostMessage.parse(
          '{"type":"state","commenting":true,"draft":{"label":"","text":"","clip":"none"},"busy":false}',
        ) as StateMsg).draft!.clip,
        DraftClip.none,
      );
    });

    test("a draft's key, kind and quote, and its screenshot", () {
      final s = HostMessage.parse(
        jsonEncode({
          'type': 'state',
          'commenting': false,
          'draft': {
            'key': 'p1',
            'kind': 'range',
            'label': '“Mon 5 km”',
            'quote': 'Mon 5 km and more',
            'text': '',
            'clip': 'ready',
          },
          'busy': false,
        }),
      ) as StateMsg;
      expect(s.draft!.key, 'p1');
      expect(s.draft!.kind, 'range');
      expect(s.draft!.quote, 'Mon 5 km and more');
      expect(s.draft!.wholeApp, isFalse);
      final whole = HostMessage.parse(
        '{"type":"state","commenting":false,"draft":{"key":"w","kind":"artifact","label":"The whole app","quote":null,"text":"","clip":"none"},"busy":false}',
      ) as StateMsg;
      expect(whole.draft!.wholeApp, isTrue);
      expect(whole.draft!.quote, isNull);
      expect(s.draft!.byPage, isFalse);
      final byPage = HostMessage.parse(
        '{"type":"state","commenting":false,"draft":{"key":"g","kind":"custom","label":"Rate this","quote":null,"text":"","clip":"none","byPage":true},"busy":false}',
      ) as StateMsg;
      expect(byPage.draft!.byPage, isTrue);
      expect(byPage.draft!.at, isNull);
      Rect? at(String json) => (HostMessage.parse(
        '{"type":"state","commenting":false,"draft":{"label":"x","text":"","clip":"none","at":$json},"busy":false}',
      ) as StateMsg).draft!.at;
      expect(at('{"x":10,"y":20.5,"w":300,"h":40}'), const Rect.fromLTWH(10, 20.5, 300, 40));
      // Anything else is no place.
      expect(at('{"x":10,"y":20,"w":-1,"h":40}'), isNull);
      expect(at('{"x":10,"y":20,"w":1}'), isNull);
      expect(at('"here"'), isNull);
      expect(
        HostMessage.parse(
          '{"type":"state","commenting":false,"draft":{"label":"x","text":"","clip":"none","byPage":"yes"},"busy":false}',
        ),
        isNull,
      );

      final clip = HostMessage.parse(
        jsonEncode({'type': 'draftClip', 'key': 'p1', 'png': 'AQID'}),
      ) as DraftClipMsg;
      expect(clip.key, 'p1');
      expect(clip.png, [1, 2, 3]);
      expect(
        HostMessage.parse('{"type":"draftClip","key":"p1","png":"%%"}'),
        isNull,
      );
      expect(HostMessage.parse('{"type":"draftClip","key":"p1"}'), isNull);
    });

    test('threads', () {
      final m = HostMessage.parse(
        jsonEncode({
          'type': 'threads',
          'threads': [
            {
              'id': 't1',
              'status': 'open',
              'summary': 'make it blue',
              'sent': true,
              'comments': 2,
              'version': 1,
              'clip': true,
            },
          ],
        }),
      ) as ThreadsMsg;
      expect(m.threads, hasLength(1));
      final t = m.threads.single;
      expect(t.id, 't1');
      expect(t.status, 'open');
      expect(t.summary, 'make it blue');
      expect(t.sent, isTrue);
      expect(t.comments, 2);
      expect(t.version, 1);
      expect(t.clip, isTrue);
    });

    test('sent, landed, notice', () {
      expect(
        (HostMessage.parse('{"type":"sent","thread":"t9"}') as Sent).thread,
        't9',
      );
      expect(
        (HostMessage.parse('{"type":"landed","version":5}') as Landed).version,
        5,
      );
      expect(
        (HostMessage.parse(
          '{"type":"notice","text":"Post failed"}',
        ) as Notice).text,
        'Post failed',
      );
    });

    test('sendFailed', () {
      expect(
        (HostMessage.parse(
          '{"type":"sendFailed","thread":"t4"}',
        ) as SendFailed).thread,
        't4',
      );
      final withText = HostMessage.parse(
        '{"type":"sendFailed","thread":"t4","text":"Offline"}',
      ) as SendFailed;
      expect(withText.text, 'Offline');
    });

    test('unknown types and broken messages are null', () {
      expect(HostMessage.parse('{"type":"later"}'), isNull);
      expect(HostMessage.parse('not json'), isNull);
      expect(HostMessage.parse('[1]'), isNull);
      expect(HostMessage.parse('{"type":"landed"}'), isNull);
      expect(HostMessage.parse('{"type":"landed","version":"5"}'), isNull);
      expect(
        HostMessage.parse(
          '{"type":"state","commenting":true,"draft":{"label":"","text":"","clip":"odd"},"busy":false}',
        ),
        isNull,
      );
    });
  });

  group('HostCall.toJs', () {
    test('renders each call', () {
      expect(
        const EnterCommentMode().toJs(),
        'window.claxHost?.enterCommentMode()',
      );
      expect(
        const ExitCommentMode().toJs(),
        'window.claxHost?.exitCommentMode()',
      );
      expect(const LoadVersion(3).toJs(), 'window.claxHost?.loadVersion(3)');
      expect(
        const LoadVersion(null).toJs(),
        'window.claxHost?.loadVersion(null)',
      );
      expect(const ReloadLatest().toJs(), 'window.claxHost?.reloadLatest()');
      expect(const SubmitDraft().toJs(), 'window.claxHost?.submitDraft()');
      expect(const CancelDraft().toJs(), 'window.claxHost?.cancelDraft()');
      expect(
        const OpenThread('t"1').toJs(),
        r'window.claxHost?.openThread("t\"1")',
      );
      expect(
        const FocusThread('t"1').toJs(),
        r'window.claxHost?.focusThread("t\"1")',
      );
      expect(
        const FocusThread(null).toJs(),
        'window.claxHost?.focusThread(null)',
      );
    });

    test('Resend and Init', () {
      expect(const Resend('t1').toJs(), 'window.claxHost?.resend("t1")');
      // init, then the screen's visibility, in the same call once the
      // shell's host is there.
      expect(
        const Init('ab"c', visible: true, waitMs: 500).toJs(),
        '(function(n,v,t){window.__claxHostVisible=v;'
        'function f(){var h=window.claxHost;'
        'if(h&&typeof h.init==="function"){h.init({nonce:n});'
        'if(typeof h.setVisible==="function")'
        'h.setVisible(window.__claxHostVisible);return}'
        r'if(t-->0)setTimeout(f,50)}f()})("ab\"c",true,10)',
      );
      expect(
        const Init('n', visible: false).toJs(),
        contains('("n",false,200)'),
      );
      // The app's platform rides on init, when it names one.
      expect(
        const Init('n', visible: true, platform: 'ios').toJs(),
        contains('h.init({nonce:n,platform:"ios"});'),
      );
      expect(
        const SetVisible(false).toJs(),
        '(function(v){window.__claxHostVisible=v;var h=window.claxHost;'
        'if(h&&typeof h.setVisible==="function")h.setVisible(v)})(false)',
      );
    });

    test(
      "the app's composer sets only the composer's text and sends its own",
      () {
        expect(
          const SetDraftText('a"b', composer: true).toJs(),
          r'window.claxHost?.setDraftText("a\"b",{composer:true})',
        );
        expect(
          const SubmitDraft(text: 'make it </b>').toJs(),
          r'window.claxHost?.submitDraft({text:"make it <\/b>"})',
        );
      },
    );

    test('SetDraftText escapes the text as a JS string', () {
      expect(
        const SetDraftText('a"b').toJs(),
        r'window.claxHost?.setDraftText("a\"b")',
      );
      expect(
        const SetDraftText('x y</script>').toJs(),
        r'window.claxHost?.setDraftText("x y<\/script>")',
      );
    });

    test(
      'AttestPointer converts physical pixels to CSS pixels in the page',
      () {
        // 10.5 x 20 logical pixels at a device pixel ratio of 2 are 21 x 40
        // physical pixels; the page divides by its own ratio and pinch scale
        // and adds the visual viewport's offset.
        expect(
          const AttestPointer(
            pointer: 7,
            phase: PointerPhase.up,
            x: 10.5,
            y: 20,
            devicePixelRatio: 2,
          ).toJs(),
          '(function(h,v,r){if(!h)return;var s=(v&&v.scale)||1;'
          'h.attestPointer({pointer:7,phase:"up",'
          'x:21.00/(r*s)+(v?v.offsetLeft:0),'
          'y:40.00/(r*s)+(v?v.offsetTop:0)})})'
          '(window.claxHost,window.visualViewport,window.devicePixelRatio||1)',
        );
      },
    );

    test('AttestPointer rejects non-finite coordinates', () {
      expect(
        () => AttestPointer(
          pointer: 1,
          phase: PointerPhase.down,
          x: double.nan,
          y: 0,
          devicePixelRatio: 1,
        ).toJs(),
        throwsArgumentError,
      );
    });
  });

  group('ShellSession', () {
    final base = Uri.parse('http://127.0.0.1:7999');
    const id = 'abcdefghjkmn';
    Uri at(String path) => Uri.parse('http://127.0.0.1:7999$path');
    final shell = at('/a/$id?app=1');
    String msg(String? nonce, [String type = 'landed']) =>
        jsonEncode({'type': type, 'version': 4, 'nonce': ?nonce});

    test('the shell URL is any app view path on the daemon origin', () {
      bool ok(String u) => ShellSession.isShellUrl(base, Uri.parse(u));
      for (final path in [
        '/a/$id?app=1',
        '/a/$id/?app=1',
        '/a/$id/v/2?app=1#x',
        '/a/$id/v/0?app=1',
        '/a/$id/about.html?app=1',
        '/a/$id/pages/about.html?app=1',
        '/a/$id/v/3/about.html?app=1',
        '/a/$id/my%20page.html?app=1',
        '/a/$id?x=2&app=1',
      ]) {
        expect(ok('http://127.0.0.1:7999$path'), isTrue, reason: path);
      }
      for (final u in [
        'http://127.0.0.1:7999/a/$id',
        'http://127.0.0.1:7999/a/$id?app=0',
        'http://127.0.0.1:7999/a/$id?app=0&app=1',
        'http://127.0.0.1:7999/a/short?app=1',
        'http://127.0.0.1:7999/a/ABCDEFGHJKMN?app=1',
        'http://127.0.0.1:7999/a/abcdefghjkmi?app=1',
        'http://127.0.0.1:7999/a/$id/bad%E0%A4.html?app=1',
        'http://127.0.0.1:7999/?app=1',
        'http://127.0.0.1:7999/b/$id?app=1',
        'http://$id.localhost:7999/a/$id?app=1',
        'http://127.0.0.1:7998/a/$id?app=1',
        'https://127.0.0.1:7999/a/$id?app=1',
        'http://u@127.0.0.1:7999/a/$id?app=1',
      ]) {
        expect(ok(u), isFalse, reason: u);
      }
      expect(ShellSession.isShellUrl(base, null), isFalse);
    });

    test(
      'forged messages are dropped: before init, without or with a wrong nonce',
      () {
        final s = ShellSession(base);
        expect(s.canCall, isFalse);
        s.documentStarted(shell);
        expect(s.accept(msg(null)), isNull, reason: 'before init');
        final init = s.documentLoaded(shell)!;
        expect(init.nonce, hasLength(32));
        expect(s.canCall, isTrue);
        expect(s.accept(msg(null)), isNull, reason: 'no nonce');
        expect(s.accept(msg('0' * 32)), isNull, reason: 'wrong nonce');
        expect(s.accept('not json'), isNull);
        expect(s.accept(msg(init.nonce)), isA<Landed>());
        expect(
          s.accept(
            jsonEncode({'type': 'sent', 'thread': 't', 'nonce': init.nonce}),
          ),
          isA<Sent>(),
        );
      },
    );

    test('the session hands its platform to every init', () {
      final s = ShellSession(base, platform: 'ios');
      s.documentStarted(shell);
      expect(s.documentLoaded(shell)!.platform, 'ios');
      final none = ShellSession(base)..documentStarted(shell);
      expect(none.documentLoaded(shell)!.platform, isNull);
    });

    test('each document gets its own nonce, once', () {
      final s = ShellSession(base);
      s.documentStarted(shell);
      final first = s.documentLoaded(shell)!;
      expect(s.documentLoaded(shell), isNull, reason: 'one nonce a document');
      s.documentStarted(shell);
      expect(s.canCall, isFalse);
      expect(s.accept(msg(first.nonce)), isNull, reason: 'the old document');
      final second = s.documentLoaded(shell)!;
      expect(second.nonce, isNot(first.nonce));
      expect(s.accept(msg(first.nonce)), isNull);
      expect(s.accept(msg(second.nonce)), isA<Landed>());
    });

    test('the document is the artifact of the URL the shell loaded at', () {
      final s = ShellSession(base);
      expect(s.document, isNull);
      s.documentStarted(shell);
      expect(s.document, isNull, reason: 'not loaded yet');
      s.documentLoaded(shell);
      expect(s.document, id);
      final other = Uri.parse('http://127.0.0.1:7999/a/zyxwvtsrqpnm?app=1');
      s.documentStarted(other);
      expect(s.document, isNull);
      s.documentLoaded(other);
      expect(s.document, 'zyxwvtsrqpnm');
      expect(ShellSession.artifactOf(base, other), 'zyxwvtsrqpnm');
      expect(
        ShellSession.artifactOf(base, Uri.parse('http://127.0.0.1:7999/x')),
        isNull,
      );
    });

    test('a document that is not the shell gets no nonce and no calls', () {
      final s = ShellSession(base);
      final page = Uri.parse('http://$id.localhost:7999/index.html');
      s.documentStarted(page);
      expect(s.documentLoaded(page), isNull);
      expect(s.canCall, isFalse);
    });

    test('a multi-page app: moving between pages keeps the channel, a reload on a page re-nonces', () {
      final s = ShellSession(base);
      s.documentStarted(shell);
      final init = s.documentLoaded(shell)!;
      // The shell pushes the page's URL when its frame moves to about.html.
      s.urlChanged(at('/a/$id/about.html?app=1'));
      expect(s.canCall, isTrue);
      expect(s.accept(msg(init.nonce)), isA<Landed>());
      // A finished-load report within the same document hands out nothing.
      expect(s.documentLoaded(at('/a/$id/about.html?app=1')), isNull);
      s.urlChanged(shell);
      expect(s.accept(msg(init.nonce)), isA<Landed>());
      s.urlChanged(at('/a/$id/v/2/about.html?app=1'));
      expect(s.accept(msg(init.nonce)), isA<Landed>());
      // Reloaded while it shows about.html: a new document and nonce.
      s.documentStarted(at('/a/$id/about.html?app=1'));
      expect(s.accept(msg(init.nonce)), isNull);
      final again = s.documentLoaded(at('/a/$id/about.html?app=1'))!;
      expect(again.nonce, isNot(init.nonce));
      expect(s.accept(msg(again.nonce)), isA<Landed>());
    });

    test('a same-document URL change never clears the session', () {
      final s = ShellSession(base);
      s.documentStarted(shell);
      final init = s.documentLoaded(shell)!;
      s.urlChanged(at('/'));
      expect(s.canCall, isTrue);
      expect(s.accept(msg(init.nonce)), isA<Landed>());
    });

    test('visibility is called only after init, and each Init carries it', () {
      final s = ShellSession(base);
      expect(s.visibilityChanged(false), isNull, reason: 'no document');
      s.documentStarted(shell);
      expect(s.visibilityChanged(true), isNull, reason: 'before init');
      final init = s.documentLoaded(shell)!;
      expect(init.visible, isTrue, reason: 'the latest visibility');
      expect(s.visibilityChanged(true), isNull, reason: 'unchanged');
      expect(s.visibilityChanged(false), isA<SetVisible>());
      // A reload: nothing until its init, which carries the visibility.
      s.documentStarted(shell);
      expect(s.visibilityChanged(true), isNull);
      expect(s.visibilityChanged(false), isNull);
      expect(s.documentLoaded(shell)!.visible, isFalse);
    });
  });

  group('visibility in the page', () {
    // Runs the calls' JavaScript in node with a fake window and timers: the
    // shell's host appears only after [hostAfter] calls, as when its chunk
    // is still loading after the document has loaded.
    Future<List<String>> run(List<HostCall> calls, {int hostAfter = 0}) async {
      final js = StringBuffer('''
const log = []; const timers = [];
globalThis.window = {};
globalThis.setTimeout = f => timers.push(f);
const host = { init: p => log.push('init'), setVisible: v => log.push('visible ' + v) };
''');
      for (var i = 0; i < calls.length; i++) {
        if (i == hostAfter) js.writeln('window.claxHost = host;');
        js.writeln('${calls[i].toJs()};');
      }
      if (hostAfter >= calls.length) js.writeln('window.claxHost = host;');
      js.writeln('while (timers.length) timers.shift()();');
      js.writeln('console.log(JSON.stringify(log));');
      final r = await Process.run('node', ['-e', js.toString()]);
      expect(r.exitCode, 0, reason: '${r.stderr}');
      return (jsonDecode(r.stdout as String) as List).cast<String>();
    }

    final node = Process.runSync('sh', ['-c', 'command -v node']).exitCode == 0;
    final skip = node ? false : 'node is not installed';

    test(
      'a change while the host loads applies after init: resumed to paused',
      () async {
        expect(
          await run(const [
            Init('n', visible: true),
            SetVisible(false),
          ], hostAfter: 2),
          ['init', 'visible false'],
        );
      },
      skip: skip,
    );

    test(
      'a change while the host loads applies after init: paused to resumed',
      () async {
        expect(
          await run(const [
            Init('n', visible: false),
            SetVisible(true),
          ], hostAfter: 2),
          ['init', 'visible true'],
        );
      },
      skip: skip,
    );

    test('only the latest of several changes applies', () async {
      expect(
        await run(const [
          Init('n', visible: true),
          SetVisible(false),
          SetVisible(true),
        ], hostAfter: 3),
        ['init', 'visible true'],
      );
    }, skip: skip);

    test('with the host there, each call applies at once, in order', () async {
      expect(await run(const [Init('n', visible: true), SetVisible(false)]), [
        'init',
        'visible true',
        'visible false',
      ]);
    }, skip: skip);
  });

  group('round 2: whole-app comments, inspections, questions', () {
    test('ComposeArtifact, OpenQuestions and Inspect render their calls', () {
      expect(
        const ComposeArtifact().toJs(),
        'window.claxHost?.composeArtifact()',
      );
      expect(const OpenQuestions().toJs(), 'window.claxHost?.openQuestions()');
      expect(
        const OpenQuestions(question: 'q1"</x').toJs(),
        'window.claxHost?.openQuestions({question:"q1\\"<\\/x"})',
      );
      expect(
        const CloseQuestions().toJs(),
        'window.claxHost?.closeQuestions()',
      );
      expect(
        const Inspect().toJs(),
        'window.claxHost?.inspect({settleMs:3000,maxBytes:1048576})',
      );
      expect(
        const Inspect(settleMs: 500, maxBytes: 2048).toJs(),
        'window.claxHost?.inspect({settleMs:500,maxBytes:2048})',
      );
    });

    test('AttestPointer passes an OS-cancelled touch as phase cancel', () {
      expect(
        const AttestPointer(
          pointer: 3,
          phase: PointerPhase.cancel,
          x: 1,
          y: 2,
          devicePixelRatio: 1,
        ).toJs(),
        contains('h.attestPointer({pointer:3,phase:"cancel",'),
      );
    });

    test('an inspection decodes its screenshot and keeps its findings', () {
      final png = [0x89, 0x50, 0x4e, 0x47, 0, 255];
      final m = HostMessage.parse(
        jsonEncode({
          'type': 'inspection',
          'png': base64Encode(png),
          'errors': [
            {'kind': 'error', 'text': 'x is not defined'},
            {'kind': 'console', 'text': 'boom'},
          ],
          'failed': ['http://127.0.0.1:7999/a/x/missing.png'],
          'text': '# Runs\n[button] Add',
          'title': 'Runs',
          'height': 1200,
        }),
      );
      final i = m as InspectionMsg;
      expect(i.error, isNull);
      expect(i.png, png);
      expect(i.errors, [
        (kind: 'error', text: 'x is not defined'),
        (kind: 'console', text: 'boom'),
      ]);
      expect(i.failed, ['http://127.0.0.1:7999/a/x/missing.png']);
      expect(i.text, '# Runs\n[button] Add');
      expect(i.title, 'Runs');
      expect(i.height, 1200);
    });

    test('RunTests renders its call, and tests decode or carry why', () {
      expect(
        const RunTests(['tests/a.test.js', 'tests/b.test.js']).toJs(),
        'window.claxHost?.runTests({files:["tests/a.test.js",'
        '"tests/b.test.js"],testMs:5000,totalMs:30000})',
      );
      final t = HostMessage.parse(
        jsonEncode({
          'type': 'tests',
          'passed': 2,
          'failed': [
            {'name': 'adds', 'error': 'Expected: 1 to be 2'},
          ],
          'skipped': 1,
        }),
      ) as TestsMsg;
      expect((t.passed, t.skipped, t.error), (2, 1, null));
      expect(t.failed, [(name: 'adds', error: 'Expected: 1 to be 2')]);
      final e = HostMessage.parse(
        '{"type":"tests","error":"the page has not loaded"}',
      ) as TestsMsg;
      expect((e.error, e.passed), ('the page has not loaded', 0));
      expect(e.failed, isEmpty);
      expect(HostMessage.parse('{"type":"tests","passed":-1}'), isNull);
      expect(
        HostMessage.parse(
          '{"type":"tests","passed":1,"failed":[{"name":1}],"skipped":0}',
        ),
        isNull,
      );
    });

    test('RunMigration renders its call, and migration decodes ok or why', () {
      expect(
        const RunMigration(from: 2, to: 3).toJs(),
        'window.claxHost?.runMigration({file:"migrate.js",from:2,to:3,'
        'totalMs:30000})',
      );
      expect(
        const RunMigration(
          file: 'lib/m.js',
          from: 1,
          to: 2,
          totalMs: 500,
        ).toJs(),
        'window.claxHost?.runMigration({file:"lib/m.js",from:1,to:2,'
        'totalMs:500})',
      );
      final ok =
          HostMessage.parse('{"type":"migration","ok":true}') as MigrationMsg;
      expect((ok.ok, ok.error), (true, null));
      final no = HostMessage.parse(
        '{"type":"migration","ok":false,"error":"the migration failed: x"}',
      ) as MigrationMsg;
      expect((no.ok, no.error), (false, 'the migration failed: x'));
      expect(HostMessage.parse('{"type":"migration","ok":false}'), isNull);
      expect(HostMessage.parse('{"type":"migration","ok":"yes"}'), isNull);
    });

    test('the migration journal: journal decodes, journalAck, a journaled '
        'RunMigration and RestoreJournal render, journalRestored decodes', () {
      expect(
        const RunMigration(from: 1, to: 2, journal: true).toJs(),
        'window.claxHost?.runMigration({file:"migrate.js",from:1,to:2,'
        'totalMs:30000,journal:true})',
      );
      final j = HostMessage.parse(
        '{"type":"journal","seq":3,"path":"data/users/u/p","before":{"v":1}}',
      ) as JournalMsg;
      expect((j.seq, j.path), (3, 'data/users/u/p'));
      expect(j.before, {'v': 1});
      final none = HostMessage.parse(
        '{"type":"journal","seq":4,"path":"a/b","before":null}',
      ) as JournalMsg;
      expect(none.before, isNull);
      expect(HostMessage.parse('{"type":"journal","seq":4,"path":1}'), isNull);
      expect(
        const JournalAck(3, ok: false).toJs(),
        'window.claxHost?.journalAck({seq:3,ok:false})',
      );
      expect(
        const RestoreJournal([
          {'path': 'a/b', 'before': null},
        ]).toJs(),
        'window.claxHost?.restoreJournal(JSON.parse('
        r'"{\"entries\":[{\"path\":\"a/b\",\"before\":null}]}"))',
      );
      expect(
        (HostMessage.parse(
          '{"type":"journalRestored","ok":true}',
        ) as JournalRestoredMsg).ok,
        isTrue,
      );
      expect(
        (HostMessage.parse(
          '{"type":"journalRestored","ok":false,"error":"refused"}',
        ) as JournalRestoredMsg).error,
        'refused',
      );
    });

    test('an inspection that failed carries only why', () {
      final i = HostMessage.parse(
        '{"type":"inspection","error":"the page has not loaded"}',
      ) as InspectionMsg;
      expect(i.error, 'the page has not loaded');
      expect(i.png, isEmpty);
      expect(i.errors, isEmpty);
      expect(i.failed, isEmpty);
    });

    test('a malformed inspection is null', () {
      for (final bad in [
        {'type': 'inspection'},
        {
          'type': 'inspection',
          'png': 'not base64!',
          'errors': [],
          'failed': [],
          'text': '',
          'title': '',
          'height': 1,
        },
        {
          'type': 'inspection',
          'png': '',
          'errors': [
            {'kind': 'error'},
          ],
          'failed': [],
          'text': '',
          'title': '',
          'height': 1,
        },
        {
          'type': 'inspection',
          'png': '',
          'errors': [],
          'failed': [1],
          'text': '',
          'title': '',
          'height': 1,
        },
        {
          'type': 'inspection',
          'png': '',
          'errors': [],
          'failed': [],
          'text': '',
          'title': '',
          'height': 1.5,
        },
      ]) {
        expect(HostMessage.parse(jsonEncode(bad)), isNull, reason: '$bad');
      }
    });

    test('questions carries how many are open and whether the sheet shows', () {
      final m = HostMessage.parse(
        '{"type":"questions","open":2,"showing":true}',
      ) as QuestionsMsg;
      expect(m.open, 2);
      expect(m.showing, isTrue);
      expect(
        (HostMessage.parse(
          '{"type":"questions","open":0}',
        ) as QuestionsMsg).showing,
        isFalse,
        reason: 'a shell without the sheet says nothing of it',
      );
      expect(
        HostMessage.parse('{"type":"questions","open":1,"showing":"yes"}'),
        isNull,
      );
      expect(HostMessage.parse('{"type":"questions"}'), isNull);
      expect(HostMessage.parse('{"type":"questions","open":-1}'), isNull);
      expect(HostMessage.parse('{"type":"questions","open":"1"}'), isNull);
    });

    test('the shell session accepts them under the nonce', () {
      final s = ShellSession(Uri.parse('http://127.0.0.1:7999'));
      final url = Uri.parse('http://127.0.0.1:7999/a/abcdefghjkmn?app=1');
      s.documentStarted(url);
      final n = s.documentLoaded(url)!.nonce;
      expect(
        s.accept(jsonEncode({'type': 'questions', 'open': 1, 'nonce': n})),
        isA<QuestionsMsg>(),
      );
      expect(
        s.accept(jsonEncode({'type': 'inspection', 'error': 'x', 'nonce': n})),
        isA<InspectionMsg>(),
      );
      expect(
        s.accept(
          jsonEncode({
            'type': 'inspection',
            'png': '%%',
            'errors': [],
            'failed': [],
            'text': '',
            'title': '',
            'height': 1,
            'nonce': n,
          }),
        ),
        isNull,
      );
    });
  });

  group('native overlays', () {
    HostMessage? parse(Map<String, Object?> m) => HostMessage.fromJson(m);
    const ask = <String, Object?>{
      'type': 'ask',
      'id': 'a1',
      'kind': 'crossRead',
      'title': 'Read “Notes”?',
      'body': 'This app wants to read another app’s data.',
      'allow': 'Allow',
      'deny': 'Don’t allow',
      'always': 'Always allow',
      'armMs': 500,
    };

    test('ask parses every kind, with and without always', () {
      final m = parse(ask) as AskMsg;
      expect(m.id, 'a1');
      expect(m.kind, AskKind.crossRead);
      expect(m.title, 'Read “Notes”?');
      expect(m.body, 'This app wants to read another app’s data.');
      expect(m.allow, 'Allow');
      expect(m.deny, 'Don’t allow');
      expect(m.always, 'Always allow');
      expect(m.armMs, 500);
      for (final k in AskKind.values) {
        expect((parse({...ask, 'kind': k.name}) as AskMsg).kind, k);
      }
      expect((parse({...ask}..remove('always')) as AskMsg).always, isNull);
    });

    test('ask refuses bad shapes', () {
      for (final bad in <Map<String, Object?>>[
        {...ask}..remove('id'),
        {...ask, 'kind': 'camera'},
        {...ask}..remove('kind'),
        {...ask, 'armMs': '500'},
        {...ask, 'armMs': -1},
        {...ask, 'title': 3},
        {...ask, 'always': true},
      ]) {
        expect(parse(bad), isNull, reason: '$bad');
      }
    });

    test('askCancel', () {
      expect((parse({'type': 'askCancel', 'id': 'a1'}) as AskCancelMsg).id, 'a1');
      expect(parse({'type': 'askCancel'}), isNull);
      expect(parse({'type': 'askCancel', 'id': 1}), isNull);
    });

    test('questions carries ids and raise, and an older shell neither', () {
      final m = parse({
        'type': 'questions',
        'open': 2,
        'showing': false,
        'ids': ['q1', 'q2'],
        'raise': true,
      }) as QuestionsMsg;
      expect(m.open, 2);
      expect(m.ids, ['q1', 'q2']);
      expect(m.raise, isTrue);
      final old = parse({'type': 'questions', 'open': 1}) as QuestionsMsg;
      expect(old.ids, isEmpty);
      expect(old.raise, isFalse);
      for (final bad in <Object?>['q1', [1], ['q1', null]]) {
        expect(
          parse({'type': 'questions', 'open': 1, 'ids': bad}),
          isNull,
          reason: '$bad',
        );
      }
      expect(parse({'type': 'questions', 'open': 1, 'raise': 'yes'}), isNull);
    });

    test('notice carries tone and an action', () {
      final plain = parse({'type': 'notice', 'text': 'Saved'}) as Notice;
      expect(plain.tone, NoticeTone.info);
      expect(plain.action, isNull);
      final m = parse({
        'type': 'notice',
        'text': 'Couldn’t post',
        'tone': 'error',
        'action': {'id': 'n1', 'label': 'Try again'},
      }) as Notice;
      expect(m.text, 'Couldn’t post');
      expect(m.tone, NoticeTone.error);
      expect(m.action, (id: 'n1', label: 'Try again'));
      expect(
        (parse({'type': 'notice', 'text': 't', 'tone': 'info', 'action': null})
                as Notice)
            .action,
        isNull,
      );
      for (final bad in <Map<String, Object?>>[
        {'type': 'notice', 'text': 't', 'tone': 'loud'},
        {'type': 'notice', 'text': 't', 'action': {'label': 'Go'}},
        {'type': 'notice', 'text': 't', 'action': 'n1'},
      ]) {
        expect(parse(bad), isNull, reason: '$bad');
      }
    });

    test('draft: none, drafting and showing', () {
      expect((parse({'type': 'draft', 'state': null}) as DraftMsg).state, isNull);
      final d = (parse({
        'type': 'draft',
        'state': {'kind': 'drafting', 'by': 'Clax', 'note': 'Adding a chart'},
      }) as DraftMsg).state as Drafting;
      expect(d.by, 'Clax');
      expect(d.note, 'Adding a chart');
      final bare = (parse({
        'type': 'draft',
        'state': {'kind': 'drafting', 'by': 'Clax'},
      }) as DraftMsg).state as Drafting;
      expect(bare.note, isNull);
      final s = (parse({
        'type': 'draft',
        'state': {
          'kind': 'showing',
          'by': 'Clax',
          'base': 4,
          'detail': '2 changes highlighted',
        },
      }) as DraftMsg).state as Showing;
      expect(s.by, 'Clax');
      expect(s.base, 4);
      expect(s.detail, '2 changes highlighted');
      for (final bad in <Object?>[
        {'kind': 'other', 'by': 'Clax'},
        {'kind': 'drafting'},
        {'kind': 'showing', 'by': 'Clax', 'base': '4', 'detail': ''},
        'drafting',
      ]) {
        expect(parse({'type': 'draft', 'state': bad}), isNull, reason: '$bad');
      }
      expect(parse({'type': 'draft'}), isNull, reason: 'state is required');
    });

    test('gone and pageWait', () {
      expect(parse({'type': 'gone'}), isA<GoneMsg>());
      expect((parse({'type': 'pageWait', 'waiting': true}) as PageWaitMsg).waiting, isTrue);
      expect((parse({'type': 'pageWait', 'waiting': false}) as PageWaitMsg).waiting, isFalse);
      expect(parse({'type': 'pageWait'}), isNull);
      expect(parse({'type': 'pageWait', 'waiting': 1}), isNull);
    });

    test('the app’s answers', () {
      expect(
        const AskAnswer('a1', AskAnswerValue.always).toJs(),
        'window.claxHost?.askAnswer("a1","always")',
      );
      expect(
        const AskAnswer('a"2', AskAnswerValue.dismiss).toJs(),
        r'window.claxHost?.askAnswer("a\"2","dismiss")',
      );
      expect(
        const NoticeAction('n1').toJs(),
        'window.claxHost?.noticeAction("n1")',
      );
      expect(const ShowDraft().toJs(), 'window.claxHost?.showDraft()');
      expect(
        const ShowDraft('Clax').toJs(),
        'window.claxHost?.showDraft("Clax")',
      );
      expect(const HideDraft().toJs(), 'window.claxHost?.hideDraft()');
    });

    test('init declares the native overlays and the platform', () {
      final js = const Init(
        'n',
        visible: true,
        native: ['ask', 'questions', 'notice', 'draft'],
        platform: 'android',
      ).toJs();
      expect(js, contains('native:["ask","questions","notice","draft"]'));
      expect(js, contains('platform:"android"'));
      expect(
        js,
        contains(
          'h.init({nonce:n,native:["ask","questions","notice","draft"],'
          'platform:"android"});',
        ),
      );
      // None declared: init is as an older app's.
      expect(
        const Init('n', visible: true).toJs(),
        contains('h.init({nonce:n});'),
      );
    });

    test('the session hands its native overlays to every init', () {
      final base = Uri.parse('http://127.0.0.1:7480');
      final shell = Uri.parse('http://127.0.0.1:7480/a/0123456789ab?app=1');
      final s = ShellSession(base, native: const ['ask'], platform: 'android')
        ..documentStarted(shell);
      final init = s.documentLoaded(shell)!;
      expect(init.native, ['ask']);
      expect(init.platform, 'android');
      final none = ShellSession(base)..documentStarted(shell);
      expect(none.documentLoaded(shell)!.native, isEmpty);
    });
  });
}
