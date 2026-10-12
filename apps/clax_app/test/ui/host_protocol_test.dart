import 'dart:convert';

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
    });

    test('Resend and Init', () {
      expect(const Resend('t1').toJs(), 'window.claxHost?.resend("t1")');
      // init, then the screen's visibility, in the same call once the
      // shell's host is there.
      expect(
        const Init('ab"c', visible: true, waitMs: 500).toJs(),
        '(function(n,v,t){function f(){var h=window.claxHost;'
        'if(h&&typeof h.init==="function"){h.init({nonce:n});'
        'if(typeof h.setVisible==="function")h.setVisible(v);return}'
        r'if(t-->0)setTimeout(f,50)}f()})("ab\"c",true,10)',
      );
      expect(
        const Init('n', visible: false).toJs(),
        contains('("n",false,200)'),
      );
      expect(
        const SetVisible(false).toJs(),
        'window.claxHost?.setVisible(false)',
      );
    });

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
      expect(
        s.visibilityChanged(false)?.toJs(),
        'window.claxHost?.setVisible(false)',
      );
      // A reload: nothing until its init, which carries the visibility.
      s.documentStarted(shell);
      expect(s.visibilityChanged(true), isNull);
      expect(s.visibilityChanged(false), isNull);
      expect(s.documentLoaded(shell)!.visible, isFalse);
    });
  });
}
