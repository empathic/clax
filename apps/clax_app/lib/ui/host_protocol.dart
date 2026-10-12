/// The protocol between the app screen and the shell's app mode
/// (`/a/<id>?app=1`, `web/shell/src/app/host.ts`): the messages the shell
/// posts on the `ClaxHost` JavaScript channel, and the `window.claxHost`
/// calls the app makes by running JavaScript in the page.
library;

import 'dart:convert';
import 'dart:math';

/// A message from the shell, posted as JSON on the `ClaxHost` channel.
sealed class HostMessage {
  const HostMessage();

  /// The message [json] encodes, or null for an unknown type or a shape
  /// the protocol does not have.
  ///
  /// This checks no nonce: the WebView's messages go through
  /// [ShellSession.accept], which does.
  static HostMessage? parse(String json) {
    try {
      final j = jsonDecode(json);
      if (j is! Map) return null;
      return fromJson(j.cast<String, Object?>());
    } on FormatException {
      return null;
    }
  }

  /// The message [m] holds, or null as for [parse].
  static HostMessage? fromJson(Map<String, Object?> m) {
    try {
      return switch (m['type']) {
        'ready' => Ready(
          artifact: m['artifact'] as String,
          version: _int(m['version']),
          latest: _int(m['latest']),
          file: m['file'] as String,
        ),
        'state' => StateMsg(
          commenting: m['commenting'] as bool,
          draft: m['draft'] == null
              ? null
              : Draft.fromJson((m['draft'] as Map).cast<String, Object?>()),
          busy: m['busy'] as bool,
        ),
        'threads' => ThreadsMsg([
          for (final t in m['threads'] as List)
            ThreadSummary.fromJson((t as Map).cast<String, Object?>()),
        ]),
        'sent' => Sent(m['thread'] as String),
        'landed' => Landed(_int(m['version'])),
        'notice' => Notice(m['text'] as String),
        'sendFailed' => SendFailed(
          m['thread'] as String,
          text: m['text'] as String? ?? '',
        ),
        _ => null,
      };
    } on TypeError {
      return null;
    }
  }
}

int _int(Object? v) => switch (v) {
  final int n => n,
  final double d when d == d.roundToDouble() => d.toInt(),
  _ => throw TypeError(),
};

/// A document loaded in the shell's frame: [version] of [artifact] at
/// [file], with [latest] the newest version the shell knows. Posted on
/// each frame load.
final class Ready extends HostMessage {
  const Ready({
    required this.artifact,
    required this.version,
    required this.latest,
    required this.file,
  });

  final String artifact;
  final int version;
  final int latest;
  final String file;

  /// Whether the view shows an older version than the latest (it does not
  /// follow new versions until it is moved back to the latest).
  bool get pinned => version < latest;
}

/// Whether the open composer's screenshot is being taken, taken, or not
/// wanted.
enum DraftClip { pending, ready, none }

/// The shell's open composer: the picked element's [label] and the
/// comment's [text].
final class Draft {
  const Draft({required this.label, required this.text, required this.clip});

  factory Draft.fromJson(Map<String, Object?> j) => Draft(
    label: j['label'] as String,
    text: j['text'] as String,
    clip: switch (j['clip']) {
      'pending' => DraftClip.pending,
      'ready' => DraftClip.ready,
      'none' => DraftClip.none,
      _ => throw TypeError(),
    },
  );

  /// What the composer is about, in a person's words, shown as is.
  final String label;
  final String text;
  final DraftClip clip;
}

/// The view's comment mode, its open composer (null when none), and
/// whether it is posting or sending.
final class StateMsg extends HostMessage {
  const StateMsg({
    required this.commenting,
    required this.draft,
    required this.busy,
  });

  final bool commenting;
  final Draft? draft;
  final bool busy;
}

/// One thread as the shell lists it.
final class ThreadSummary {
  const ThreadSummary({
    required this.id,
    required this.status,
    required this.summary,
    required this.sent,
    required this.comments,
    required this.version,
    this.label = '',
  });

  factory ThreadSummary.fromJson(Map<String, Object?> j) => ThreadSummary(
    id: j['id'] as String,
    status: j['status'] as String,
    summary: j['summary'] as String,
    sent: j['sent'] as bool,
    comments: _int(j['comments']),
    version: _int(j['version']),
    label: j['label'] as String? ?? '',
  );

  final String id;

  /// `open` or `resolved`.
  final String status;

  /// The first comment, shortened.
  final String summary;

  /// Whether the thread was sent to the agent.
  final bool sent;
  final int comments;

  /// The version the thread was made on.
  final int version;

  /// What the thread is anchored to, in a person's words; empty when the
  /// shell gives none.
  final String label;
}

/// The view's threads changed.
final class ThreadsMsg extends HostMessage {
  const ThreadsMsg(this.threads);

  final List<ThreadSummary> threads;
}

/// Send posted [thread] and sent it to the agent.
final class Sent extends HostMessage {
  const Sent(this.thread);

  final String thread;
}

/// Send posted [thread] but could not send it to the agent; [Resend]
/// tries again.
final class SendFailed extends HostMessage {
  const SendFailed(this.thread, {this.text = ''});

  final String thread;

  /// Why, as the shell words it.
  final String text;
}

/// The view followed a newer version, and its frame loaded [version].
final class Landed extends HostMessage {
  const Landed(this.version);

  final int version;
}

/// A notice the shell shows (a post that failed, …).
final class Notice extends HostMessage {
  const Notice(this.text);

  final String text;
}

/// A call into the page's `window.claxHost`, run as JavaScript. Each is a
/// no-op on a page without it.
sealed class HostCall {
  const HostCall();

  /// The JavaScript that makes the call.
  String toJs();
}

/// The JavaScript string literal for [s]. Besides JSON's escapes it
/// escapes U+2028, U+2029 and `</`, so the literal is safe anywhere in a
/// script.
String jsString(String s) =>
    jsonEncode(s)
        .replaceAll(' ', r' ')
        .replaceAll(' ', r' ')
        .replaceAll('</', r'<\/');

/// Turns comment mode on (nothing when it is on).
final class EnterCommentMode extends HostCall {
  const EnterCommentMode();
  @override
  String toJs() => 'window.claxHost?.enterCommentMode()';
}

/// Turns comment mode off (nothing when it is off).
final class ExitCommentMode extends HostCall {
  const ExitCommentMode();
  @override
  String toJs() => 'window.claxHost?.exitCommentMode()';
}

/// Shows version [n], or the latest (following new versions) for null.
final class LoadVersion extends HostCall {
  const LoadVersion(this.n);
  final int? n;
  @override
  String toJs() => 'window.claxHost?.loadVersion(${n ?? 'null'})';
}

/// Loads the latest version in place, keeping page, fragment and scroll.
final class ReloadLatest extends HostCall {
  const ReloadLatest();
  @override
  String toJs() => 'window.claxHost?.reloadLatest()';
}

/// Replaces the open composer's text.
final class SetDraftText extends HostCall {
  const SetDraftText(this.text);
  final String text;
  @override
  String toJs() => 'window.claxHost?.setDraftText(${jsString(text)})';
}

/// Presses Send on the open composer.
final class SubmitDraft extends HostCall {
  const SubmitDraft();
  @override
  String toJs() => 'window.claxHost?.submitDraft()';
}

/// Closes the open composer.
final class CancelDraft extends HostCall {
  const CancelDraft();
  @override
  String toJs() => 'window.claxHost?.cancelDraft()';
}

/// Selects thread [id] and brings it into view.
final class OpenThread extends HostCall {
  const OpenThread(this.id);
  final String id;
  @override
  String toJs() => 'window.claxHost?.openThread(${jsString(id)})';
}

/// Hands the shell this document's [nonce], then tells it whether the app
/// shows it ([visible], `setVisible`). The shell posts nothing before
/// init, and every message after it carries the nonce. The shell's
/// `claxHost` may appear a moment after the document loads, so the call
/// waits for it (up to [waitMs]) in the page.
final class Init extends HostCall {
  const Init(this.nonce, {required this.visible, this.waitMs = 10000});
  final String nonce;
  final bool visible;
  final int waitMs;
  @override
  String toJs() =>
      '(function(n,v,t){function f(){var h=window.claxHost;'
      'if(h&&typeof h.init==="function"){h.init({nonce:n});'
      'if(typeof h.setVisible==="function")h.setVisible(v);return}'
      'if(t-->0)setTimeout(f,50)}f()})'
      '(${jsString(nonce)},$visible,${waitMs ~/ 50})';
}

/// Whether the app shows the page: the WebView reports its document as
/// visible or not on its own terms (hidden in some hosts while on screen),
/// so in app mode the shell takes its visibility from the app.
final class SetVisible extends HostCall {
  const SetVisible(this.visible);
  final bool visible;
  @override
  String toJs() => 'window.claxHost?.setVisible($visible)';
}

/// Sends thread [id] to the agent again, after [SendFailed].
final class Resend extends HostCall {
  const Resend(this.id);
  final String id;
  @override
  String toJs() => 'window.claxHost?.resend(${jsString(id)})';
}

/// Whether an [AttestPointer] is a press or a release.
enum PointerPhase { down, up }

/// A real finger pressed or lifted on the WebView at ([x], [y]) logical
/// pixels from its top left, on a screen of [devicePixelRatio].
///
/// The shell counts it as the finger's arrival on the content frame
/// (`attestPointer`), which a long-press's pick needs when the WebView
/// sends no click after it. The page takes CSS pixels of its layout
/// viewport, so the call converts there: the physical pixels divided by
/// the page's own device pixel ratio and pinch scale, plus the visual
/// viewport's offset.
final class AttestPointer extends HostCall {
  const AttestPointer({
    required this.pointer,
    required this.phase,
    required this.x,
    required this.y,
    required this.devicePixelRatio,
  });

  /// The pointer's ID: a release counts only after its press was attested.
  final int pointer;
  final PointerPhase phase;
  final double x;
  final double y;
  final double devicePixelRatio;

  @override
  String toJs() {
    final px = x * devicePixelRatio;
    final py = y * devicePixelRatio;
    if (!px.isFinite || !py.isFinite) {
      throw ArgumentError('AttestPointer needs finite coordinates');
    }
    return '(function(h,v,r){if(!h)return;var s=(v&&v.scale)||1;'
        'h.attestPointer({pointer:$pointer,phase:"${phase.name}",'
        'x:${px.toStringAsFixed(2)}/(r*s)+(v?v.offsetLeft:0),'
        'y:${py.toStringAsFixed(2)}/(r*s)+(v?v.offsetTop:0)})})'
        '(window.claxHost,window.visualViewport,window.devicePixelRatio||1)';
  }
}

/// The host's side of one WebView's documents: which top-level document is
/// the shell, its nonce, and which messages are the shell's.
///
/// - The top level may only be the shell's app view on [base]'s origin
///   ([isShellUrl], the rule the shell installs its host by).
/// - Each shell document gets a fresh random nonce once it has loaded
///   ([documentLoaded] returns the [Init] call carrying it); a URL change
///   inside the document (the shell moving between the app's pages or
///   versions by history) keeps it.
/// - A message is the shell's only when it carries the current nonce
///   ([accept]): every frame of the page can reach the JavaScript channel,
///   so anything else is dropped, as is everything before [Init] and
///   after a new document starts.
/// - `claxHost` is called only in a shell document that was handed its
///   nonce ([canCall]).
class ShellSession {
  ShellSession(this.base, {Random? random})
    : _random = random ?? Random.secure();

  /// The daemon's base URL (`http://127.0.0.1:<port>`).
  final Uri base;
  final Random _random;
  String? _nonce;
  var _shell = false;
  var _visible = true;

  /// An artifact ID, as the shell's `parseShellPath` takes it
  /// (`web/shell/src/route.ts`).
  static final _id = RegExp(r'^[0-9a-hj-km-np-tv-z]{12}$');

  /// Whether [url] is the shell's app view on [base]'s origin: the path
  /// `/a/<id>[/v/<n>][/<file>]` (`parseShellPath` in
  /// `web/shell/src/route.ts`: a valid ID, an optional version, then any
  /// file segments that percent-decode) with `app=1` first in its query
  /// (`appQuery`), which is where the shell installs `window.claxHost`.
  static bool isShellUrl(Uri base, Uri? url) {
    if (url == null ||
        url.scheme != base.scheme ||
        url.host.toLowerCase() != base.host.toLowerCase() ||
        url.port != base.port ||
        url.userInfo.isNotEmpty) {
      return false;
    }
    if (url.queryParametersAll['app']?.first != '1') return false;
    final segs = url.path.split('/');
    if (segs.length < 3 || segs[0] != '' || segs[1] != 'a') return false;
    if (!_id.hasMatch(segs[2])) return false;
    try {
      for (final s in segs.skip(3)) {
        Uri.decodeComponent(s);
      }
    } on FormatException {
      return false;
    } on ArgumentError {
      return false;
    }
    return true;
  }

  /// Whether calls may be made now.
  bool get canCall => _shell && _nonce != null;

  /// A new top-level document started loading at [url]: nothing is
  /// accepted or called until it has loaded.
  void documentStarted(Uri? url) {
    _nonce = null;
    _shell = isShellUrl(base, url);
  }

  /// The top-level URL changed inside the document (the shell's history
  /// push or replace). The document, and so its host and nonce, stay.
  void urlChanged(Uri? url) {}

  /// The top-level document loaded at [url]: the call handing the shell
  /// its nonce, or null when it is not the shell or already has one (a
  /// document is handed one nonce).
  Init? documentLoaded(Uri? url) {
    if (_nonce != null) return null;
    _shell = isShellUrl(base, url);
    if (!_shell) return null;
    final bytes = List<int>.generate(16, (_) => _random.nextInt(256));
    final n = [for (final b in bytes) b.toRadixString(16).padLeft(2, '0')]
        .join();
    _nonce = n;
    return Init(n, visible: _visible);
  }

  /// The app shows the page ([visible]) or not: the call telling the
  /// shell, or null before the document's [Init] (which carries it) or
  /// when nothing changed.
  SetVisible? visibilityChanged(bool visible) {
    if (visible == _visible) return null;
    _visible = visible;
    return canCall ? SetVisible(visible) : null;
  }

  /// The message [raw] holds when it carries the current nonce; null for
  /// anything else.
  HostMessage? accept(String raw) {
    final n = _nonce;
    if (n == null || !_shell) return null;
    try {
      final j = jsonDecode(raw);
      if (j is! Map || j['nonce'] != n) return null;
      return HostMessage.fromJson(j.cast<String, Object?>());
    } on FormatException {
      return null;
    }
  }
}
