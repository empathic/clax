/// The protocol between the app screen and the shell's app mode
/// (`/a/<id>?app=1`, `web/shell/src/app/host.ts`): the messages the shell
/// posts on the `ClaxHost` JavaScript channel, and the `window.claxHost`
/// calls the app makes by running JavaScript in the page.
library;

import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';
import 'dart:ui' show Color, Rect;

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
        'draftClip' => DraftClipMsg(
          m['key'] as String,
          base64Decode(m['png'] as String),
        ),
        'sent' => Sent(m['thread'] as String),
        'landed' => Landed(_int(m['version'])),
        'newer' => NewerMsg(_int(m['version'])),
        'history' => HistoryMsg(back: m['back'] as bool),
        'notice' => Notice.fromJson(m),
        'sendFailed' => SendFailed(
          m['thread'] as String,
          text: m['text'] as String? ?? '',
        ),
        'inspection' => InspectionMsg.fromJson(m),
        'tests' => TestsMsg.fromJson(m),
        'migration' => MigrationMsg.fromJson(m),
        'journal' => JournalMsg(
          _int(m['seq']),
          m['path'] as String,
          m['before'] == null
              ? null
              : (m['before'] as Map).cast<String, Object?>(),
        ),
        'journalRestored' =>
          m['ok'] == true
              ? const JournalRestoredMsg(null)
              : JournalRestoredMsg(m['error'] as String),
        'questions' => QuestionsMsg(
          _count(m['open']),
          showing: (m['showing'] ?? false) as bool,
          ids: _strings(m['ids'] ?? const <Object?>[]),
          raise: (m['raise'] ?? false) as bool,
        ),
        'device' => DeviceRequest.fromJson(m),
        'chrome' => ChromeMsg(_hex(m['top']), _hex(m['bottom'])),
        'ask' => AskMsg.fromJson(m),
        'askCancel' => AskCancelMsg(m['id'] as String),
        'draft' => DraftMsg.fromJson(m),
        'gone' => const GoneMsg(),
        'pageWait' => PageWaitMsg(waiting: m['waiting'] as bool),
        _ => null,
      };
    } on TypeError {
      return null;
    } on FormatException {
      return null;
    }
  }
}

/// The strings [v] lists; anything else is a [TypeError].
List<String> _strings(Object? v) =>
    List<String>.unmodifiable([for (final s in v as List) s as String]);

/// The value of [values] named [v]; anything else is a [TypeError].
T _named<T extends Enum>(List<T> values, Object? v) {
  for (final e in values) {
    if (e.name == v) return e;
  }
  throw TypeError();
}

int _count(Object? v) {
  final n = _int(v);
  if (n < 0) throw TypeError();
  return n;
}

/// The opaque colour `#rrggbb` [v] names; anything else is a [TypeError].
Color _hex(Object? v) {
  if (v is! String || !RegExp(r'^#[0-9a-fA-F]{6}$').hasMatch(v)) {
    throw TypeError();
  }
  return Color(0xFF000000 | int.parse(v.substring(1), radix: 16));
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

/// The shell's open composer, which the app's composer shows and sends:
/// what it is on ([kind], [label], [quote]) and the comment's [text].
final class Draft {
  const Draft({
    required this.label,
    required this.text,
    required this.clip,
    this.key = '',
    this.kind = 'element',
    this.quote,
    this.byPage = false,
    this.at,
  });

  factory Draft.fromJson(Map<String, Object?> j) => Draft(
    key: j['key'] as String? ?? '',
    kind: j['kind'] as String? ?? 'element',
    label: j['label'] as String,
    quote: j['quote'] as String?,
    byPage: j['byPage'] as bool? ?? false,
    at: _rectOf(j['at']),
    text: j['text'] as String,
    clip: switch (j['clip']) {
      'pending' => DraftClip.pending,
      'ready' => DraftClip.ready,
      'none' => DraftClip.none,
      _ => throw TypeError(),
    },
  );

  /// The composer's identity: a new one for each pick. Its screenshot
  /// comes under it ([DraftClipMsg]).
  final String key;

  /// The anchor's kind: `element`, `range` (selected text), `area` (a
  /// drawn rectangle), `custom` (a spot the page named) or `artifact`
  /// (the whole app).
  final String kind;

  /// What the composer is about, in a person's words, shown as is.
  final String label;

  /// The text it quotes as a reader sees it, for the app to show in its
  /// own quote marks (no quote marks of the page's in it); null when it
  /// quotes none.
  final String? quote;
  final String text;
  final DraftClip clip;

  /// Whether the app's page opened this composer (its comments
  /// capability) rather than the person's pick: its [label] is the page's
  /// own words.
  final bool byPage;

  /// Where the viewer's pick is in the WebView, in logical pixels from its
  /// top left: where the shell last placed the pick's anchor, as it places
  /// the threads' pins, so a later state message moves it as the page
  /// scrolls. Null for a composer the page opened, one on the whole app, or
  /// a pick the shell has not placed (yet, or at all).
  final Rect? at;

  /// Whether this is a comment on the whole app.
  bool get wholeApp => kind == 'artifact';

  /// [v] as a rectangle {x, y, w, h} of finite numbers with no negative
  /// size, else null.
  static Rect? _rectOf(Object? v) {
    if (v case {
      'x': final num x,
      'y': final num y,
      'w': final num w,
      'h': final num h,
    } when [x, y, w, h].every((n) => n.isFinite) && w >= 0 && h >= 0) {
      return Rect.fromLTWH(x.toDouble(), y.toDouble(), w.toDouble(), h.toDouble());
    }
    return null;
  }
}

/// The screenshot of open composer [key] (a PNG), posted once for each
/// screenshot it takes.
final class DraftClipMsg extends HostMessage {
  const DraftClipMsg(this.key, this.png);

  final String key;
  final Uint8List png;
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
    this.clip = false,
  });

  factory ThreadSummary.fromJson(Map<String, Object?> j) => ThreadSummary(
    id: j['id'] as String,
    status: j['status'] as String,
    summary: j['summary'] as String,
    sent: j['sent'] as bool,
    comments: _int(j['comments']),
    version: _int(j['version']),
    label: j['label'] as String? ?? '',
    clip: j['clip'] as bool? ?? false,
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

  /// Whether the thread has a screenshot (`ClaxApi.clip`).
  final bool clip;
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

/// A newer [version] than the one shown is published; the shell loads it
/// only on [ReloadLatest] (a view pinned to an earlier version is offered
/// nothing).
final class NewerMsg extends HostMessage {
  const NewerMsg(this.version);

  final int version;
}

/// Whether the app's page has a page or fragment of its own to go [back]
/// to ([GoBack]).
final class HistoryMsg extends HostMessage {
  const HistoryMsg({required this.back});

  final bool back;
}

/// Whether a [Notice] tells of something done or something that failed.
enum NoticeTone { info, error }

/// A notice the shell shows (a post that failed, …), or, with `notice`
/// declared native ([Init.native]), asks the app to show in its place.
/// With [action], the notice offers it, and pressing it calls
/// [NoticeAction] with its [id].
final class Notice extends HostMessage {
  const Notice(this.text, {this.tone = NoticeTone.info, this.action});

  /// The message [m] holds; a [TypeError] for any other shape. A shell that
  /// sends no `tone` sends an [NoticeTone.info] notice.
  factory Notice.fromJson(Map<String, Object?> m) {
    final a = m['action'];
    return Notice(
      m['text'] as String,
      tone: _named(NoticeTone.values, m['tone'] ?? 'info'),
      action: a == null
          ? null
          : (
              id: (a as Map)['id'] as String,
              label: a['label'] as String,
            ),
    );
  }

  final String text;
  final NoticeTone tone;
  final ({String id, String label})? action;
}

/// The inspect view's answer to [Inspect]: the page as it settled, or
/// [error], why there is none (then every other field is empty).
final class InspectionMsg extends HostMessage {
  const InspectionMsg({
    required this.png,
    required this.errors,
    required this.failed,
    required this.text,
    required this.title,
    required this.height,
  }) : error = null;

  InspectionMsg.failed(String this.error)
    : png = Uint8List(0),
      errors = const [],
      failed = const [],
      text = '',
      title = '',
      height = 0;

  /// The message [m] holds; a [TypeError] or [FormatException] for any
  /// other shape (a field missing or of the wrong type, `png` not base64).
  factory InspectionMsg.fromJson(Map<String, Object?> m) {
    final error = m['error'];
    if (error is String) return InspectionMsg.failed(error);
    return InspectionMsg(
      png: base64Decode(m['png'] as String),
      errors: [
        for (final e in m['errors'] as List)
          (kind: (e as Map)['kind'] as String, text: e['text'] as String),
      ],
      failed: [for (final f in m['failed'] as List) f as String],
      text: m['text'] as String,
      title: m['title'] as String,
      height: _int(m['height']),
    );
  }

  /// The page's viewport as a PNG; empty with [error].
  final Uint8List png;

  /// The page's newest errors since its first script: [kind] is `error`,
  /// `unhandledrejection` or `console`.
  final List<({String kind, String text})> errors;

  /// The URLs of the resources the page failed to load.
  final List<String> failed;

  /// An outline of what the page shows.
  final String text;
  final String title;

  /// The page's scroll height, in CSS pixels.
  final int height;

  /// Why the inspection failed, when it did.
  final String? error;
}

/// The inspect view's answer to [RunTests]: how many tests [passed], each
/// that [failed] (or test file that did not load) with why, and how many
/// were [skipped] for lack of time; or [error], why there was no run (then
/// the rest is empty).
final class TestsMsg extends HostMessage {
  const TestsMsg({
    required this.passed,
    required this.failed,
    required this.skipped,
  }) : error = null;

  const TestsMsg.failed(String this.error)
    : passed = 0,
      failed = const [],
      skipped = 0;

  /// The message [m] holds; a [TypeError] for any other shape.
  factory TestsMsg.fromJson(Map<String, Object?> m) {
    final error = m['error'];
    if (error is String) return TestsMsg.failed(error);
    return TestsMsg(
      passed: _count(m['passed']),
      failed: [
        for (final f in m['failed'] as List)
          (name: (f as Map)['name'] as String, error: f['error'] as String),
      ],
      skipped: _count(m['skipped']),
    );
  }

  final int passed;
  final List<({String name, String error})> failed;
  final int skipped;

  /// Why there was no run, when there was none.
  final String? error;
}

/// The inspect view's answer to [RunMigration]: the migration finished
/// ([ok]), or [error], why not (it threw, did not load, ran out of time, or
/// there was no run).
final class MigrationMsg extends HostMessage {
  const MigrationMsg.ok() : ok = true, error = null;
  const MigrationMsg.failed(String this.error) : ok = false;

  /// The message [m] holds; a [TypeError] for any other shape.
  factory MigrationMsg.fromJson(Map<String, Object?> m) => m['ok'] == true
      ? const MigrationMsg.ok()
      : m['ok'] == false
      ? MigrationMsg.failed(m['error'] as String)
      : throw TypeError();

  final bool ok;

  /// Why the migration did not finish, when it did not.
  final String? error;
}

/// A journaled migration's write is waiting (see [RunMigration.journal]):
/// the document at [path] held [before] (null: none). [JournalAck] answers.
final class JournalMsg extends HostMessage {
  const JournalMsg(this.seq, this.path, this.before);

  final int seq;
  final String path;
  final Map<String, Object?>? before;
}

/// The answer to [RestoreJournal]: done, or [error], why not.
final class JournalRestoredMsg extends HostMessage {
  const JournalRestoredMsg(this.error);

  final String? error;

  bool get ok => error == null;
}

/// How many of the owner's questions about this app are [open], and
/// whether the shell's questions sheet is [showing]; posted whenever either
/// changes.
final class QuestionsMsg extends HostMessage {
  const QuestionsMsg(
    this.open, {
    this.showing = false,
    this.ids = const [],
    this.raise = false,
  });

  final int open;

  /// Whether the questions sheet is up over the app (false when the shell
  /// does not say).
  final bool showing;

  /// The open questions' IDs (empty when the shell does not say).
  final List<String> ids;

  /// Whether the shell would raise its sheet now: with `questions` declared
  /// native ([Init.native]), the app shows its own.
  final bool raise;
}

/// The page's background at the middle of the viewport's [top] and
/// [bottom] edges, which the system's status and navigation bars are
/// painted in; posted on the page's first paint and whenever either changes
/// (sampled by the page's bridge, `web/bridge/src/chrome.ts`).
final class ChromeMsg extends HostMessage {
  const ChromeMsg(this.top, this.bottom);

  final Color top, bottom;
}

/// What a permission prompt ([AskMsg]) asks for.
enum AskKind { capability, download, crossRead, sample, comments }

/// A permission prompt at the head of the shell's queue, for the app to show
/// when `ask` is declared native ([Init.native]) and answer with
/// [AskAnswer] under the same [id]. [allow] counts only from [armMs]
/// milliseconds after the prompt first shows. [always], when given, labels
/// a choice that allows and keeps the grant.
final class AskMsg extends HostMessage {
  const AskMsg({
    required this.id,
    required this.kind,
    required this.title,
    required this.body,
    required this.allow,
    required this.deny,
    this.always,
    required this.armMs,
  });

  /// The message [m] holds; a [TypeError] for any other shape.
  factory AskMsg.fromJson(Map<String, Object?> m) => AskMsg(
    id: m['id'] as String,
    kind: _named(AskKind.values, m['kind']),
    title: m['title'] as String,
    body: m['body'] as String,
    allow: m['allow'] as String,
    deny: m['deny'] as String,
    always: m['always'] as String?,
    armMs: _count(m['armMs']),
  );

  final String id;
  final AskKind kind;
  final String title, body, allow, deny;
  final String? always;
  final int armMs;
}

/// The prompt [id] ([AskMsg]) is no longer asked (the view unloaded or the
/// page went away): the app takes it down, and an answer to it changes
/// nothing.
final class AskCancelMsg extends HostMessage {
  const AskCancelMsg(this.id);

  final String id;
}

/// What the draft banner says: an agent's draft is being made, or one is
/// shown in place of the app.
sealed class DraftState {
  const DraftState();

  /// The state [j] holds; a [TypeError] for any other shape.
  static DraftState fromJson(Map<String, Object?> j) => switch (j['kind']) {
    'drafting' => Drafting(j['by'] as String, j['note'] as String?),
    'showing' => Showing(
      j['by'] as String,
      _int(j['base']),
      j['detail'] as String,
    ),
    _ => throw TypeError(),
  };
}

/// Agent [by] (its display name) is drafting a change, with its [note]
/// when it left one; [ShowDraft] shows the draft.
final class Drafting extends DraftState {
  const Drafting(this.by, this.note);

  final String by;
  final String? note;
}

/// Agent [by]'s draft, made on version [base], is shown in place of the
/// app; [detail] says what changed. [HideDraft] goes back.
final class Showing extends DraftState {
  const Showing(this.by, this.base, this.detail);

  final String by;
  final int base;
  final String detail;
}

/// The draft banner's text changed, to [state] (null: no banner), for the
/// app to show when `draft` is declared native ([Init.native]).
final class DraftMsg extends HostMessage {
  const DraftMsg(this.state);

  /// The message [m] holds; a [TypeError] for any other shape (`state`
  /// must be present, null or a state).
  factory DraftMsg.fromJson(Map<String, Object?> m) {
    if (!m.containsKey('state')) throw TypeError();
    final s = m['state'];
    return DraftMsg(
      s == null ? null : DraftState.fromJson((s as Map).cast<String, Object?>()),
    );
  }

  final DraftState? state;
}

/// The artifact the view shows was deleted.
final class GoneMsg extends HostMessage {
  const GoneMsg();
}

/// Comment mode or a page's `use()` started ([waiting]) or stopped waiting
/// on a slow page.
final class PageWaitMsg extends HostMessage {
  const PageWaitMsg({required this.waiting});

  final bool waiting;
}

/// A device capability call of the page (`notify`, `share`, `files`,
/// `camera`, `location`, `home`; `web/shell/src/caps/device-link.ts`), for the app
/// to make and answer with [DeviceResult] under the same [id].
final class DeviceRequest extends HostMessage {
  const DeviceRequest({
    required this.id,
    required this.cap,
    required this.method,
    required this.args,
  });

  /// The message [m] holds; a [TypeError] for any other shape.
  factory DeviceRequest.fromJson(Map<String, Object?> m) => DeviceRequest(
    id: m['id'] as String,
    cap: m['cap'] as String,
    method: m['method'] as String,
    args: List<Object?>.unmodifiable(m['args'] as List),
  );

  final String id;
  final String cap;
  final String method;

  /// The call's arguments, as decoded JSON.
  final List<Object?> args;
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

/// How the app answered a permission prompt: [always] allows and keeps the
/// grant as the shell's own "always" choice does; [dismiss] (Escape, a tap
/// outside) answers neither way and stores nothing.
enum AskAnswerValue { allow, deny, always, dismiss }

/// The app's answer to prompt [id] ([AskMsg]); the shell ignores it after
/// [AskCancelMsg] or for a prompt it does not have.
final class AskAnswer extends HostCall {
  const AskAnswer(this.id, this.answer);
  final String id;
  final AskAnswerValue answer;
  @override
  String toJs() =>
      'window.claxHost?.askAnswer(${jsString(id)},"${answer.name}")';
}

/// Runs the action [id] of a [Notice].
final class NoticeAction extends HostCall {
  const NoticeAction(this.id);
  final String id;
  @override
  String toJs() => 'window.claxHost?.noticeAction(${jsString(id)})';
}

/// Shows a draft in place of the app, as the draft banner's Show draft
/// does: the draft of the agent a [DraftState] names [by], or the one the
/// banner offers when null.
final class ShowDraft extends HostCall {
  const ShowDraft([this.by]);
  final String? by;
  @override
  String toJs() => by == null
      ? 'window.claxHost?.showDraft()'
      : 'window.claxHost?.showDraft(${jsString(by!)})';
}

/// Goes back from a shown draft to the app, as the draft banner's Back
/// does.
final class HideDraft extends HostCall {
  const HideDraft();
  @override
  String toJs() => 'window.claxHost?.hideDraft()';
}

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

/// Moves the app's page back to the page or fragment it showed before, in
/// place ([HistoryMsg]).
final class GoBack extends HostCall {
  const GoBack();
  @override
  String toJs() => 'window.claxHost?.back()';
}

/// Replaces the open composer's text. Without [composer] a focused text
/// field of the shell's questions sheet takes it instead (dictation into
/// a question); with it, only the composer does (the app's composer).
final class SetDraftText extends HostCall {
  const SetDraftText(this.text, {this.composer = false});
  final String text;
  final bool composer;
  @override
  String toJs() => composer
      ? 'window.claxHost?.setDraftText(${jsString(text)},{composer:true})'
      : 'window.claxHost?.setDraftText(${jsString(text)})';
}

/// Presses Send on the open composer, with [text] as its comment when
/// given (the app's composer): the shell posts the thread on the pick's
/// anchor and screenshot, waiting for the screenshot as Send does.
final class SubmitDraft extends HostCall {
  const SubmitDraft({this.text});
  final String? text;
  @override
  String toJs() => text == null
      ? 'window.claxHost?.submitDraft()'
      : 'window.claxHost?.submitDraft({text:${jsString(text!)}})';
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

/// The app's own thread view opened on thread [id] (its area stays
/// outlined while it is open), or closed (null).
final class FocusThread extends HostCall {
  const FocusThread(this.id);
  final String? id;
  @override
  String toJs() =>
      'window.claxHost?.focusThread(${id == null ? 'null' : jsString(id!)})';
}

/// The page global holding the latest visibility the app gave this
/// document, which [Init] applies once the shell's host is there.
const _visibleGlobal = 'window.__claxHostVisible';

/// Hands the shell this document's [nonce], then tells it whether the app
/// shows it (`setVisible`). The shell posts nothing before init, and every
/// message after it carries the nonce. The shell's `claxHost` may appear a
/// moment after the document loads, so the call waits for it (up to
/// [waitMs]) in the page. It records [visible] in a page global that each
/// later [SetVisible] overwrites, and applies the global's value after
/// init: a change made while the host was loading is not lost, and only
/// the latest applies.
final class Init extends HostCall {
  const Init(
    this.nonce, {
    required this.visible,
    this.waitMs = 10000,
    this.native = const [],
    this.platform,
  });
  final String nonce;
  final bool visible;
  final int waitMs;

  /// The overlays the app draws itself (any of `ask`, `questions`,
  /// `notice` and `draft`), passed to `init` when there are any: the shell
  /// posts those as messages and keeps its web version of every other.
  final List<String> native;

  /// The app's platform (`ios`, `android`, `macos`), passed to `init` when
  /// given: on `ios` the shell makes camera, files, share and location
  /// calls through WebKit, asking the app only for the grant.
  final String? platform;

  @override
  String toJs() =>
      '(function(n,v,t){$_visibleGlobal=v;'
      'function f(){var h=window.claxHost;'
      'if(h&&typeof h.init==="function"){h.init({nonce:n'
      '${native.isEmpty ? '' : ',native:[${native.map(jsString).join(',')}]'}'
      '${platform == null ? '' : ',platform:${jsString(platform!)}'}});'
      'if(typeof h.setVisible==="function")'
      'h.setVisible($_visibleGlobal);return}'
      'if(t-->0)setTimeout(f,50)}f()})'
      '(${jsString(nonce)},$visible,${waitMs ~/ 50})';
}

/// Whether the app shows the page: the WebView reports its document as
/// visible or not on its own terms (hidden in some hosts while on screen),
/// so in app mode the shell takes its visibility from the app. Recorded in
/// the page global [Init] reads, so it holds even before the shell's host
/// has loaded.
final class SetVisible extends HostCall {
  const SetVisible(this.visible);
  final bool visible;
  @override
  String toJs() =>
      '(function(v){$_visibleGlobal=v;var h=window.claxHost;'
      'if(h&&typeof h.setVisible==="function")h.setVisible(v)})($visible)';
}

/// Sends thread [id] to the agent again, after [SendFailed].
final class Resend extends HostCall {
  const Resend(this.id);
  final String id;
  @override
  String toJs() => 'window.claxHost?.resend(${jsString(id)})';
}

/// Opens the composer on the whole app (no screenshot), unless one holding
/// typed text is open; the shell's `state` reports it like a pick.
final class ComposeArtifact extends HostCall {
  const ComposeArtifact();
  @override
  String toJs() => 'window.claxHost?.composeArtifact()';
}

/// Shows the open questions about the app in the shell's sheet, docked
/// over it; with [question] (an ID), with that question's card in focus as
/// soon as the shell knows it open.
final class OpenQuestions extends HostCall {
  const OpenQuestions({this.question});
  final String? question;
  @override
  String toJs() => question == null
      ? 'window.claxHost?.openQuestions()'
      : 'window.claxHost?.openQuestions({question:${jsString(question!)}})';
}

/// Closes the questions sheet, and drops a waiting [OpenQuestions].
final class CloseQuestions extends HostCall {
  const CloseQuestions();
  @override
  String toJs() => 'window.claxHost?.closeQuestions()';
}

/// In the inspect view (`?app=1&inspect=1`), asks for an [InspectionMsg]
/// once the page has been still for 300 ms, waiting [settleMs] at most,
/// its screenshot at most [maxBytes]. A later call replaces an earlier one;
/// anywhere else the shell answers an inspection with an error.
final class Inspect extends HostCall {
  const Inspect({this.settleMs = 3000, this.maxBytes = 1 << 20});
  final int settleMs;
  final int maxBytes;
  @override
  String toJs() =>
      'window.claxHost?.inspect({settleMs:$settleMs,maxBytes:$maxBytes})';
}

/// In the inspect view, asks the page to run the draft's test [files]
/// (`tests/<name>.test.js`), each test for at most [testMs], the run for at
/// most [totalMs]: a [TestsMsg] answers. A later call replaces an earlier
/// one; anywhere else the shell answers with an error.
final class RunTests extends HostCall {
  const RunTests(this.files, {this.testMs = 5000, this.totalMs = 30000});
  final List<String> files;
  final int testMs;
  final int totalMs;
  @override
  String toJs() =>
      'window.claxHost?.runTests({files:[${files.map(jsString).join(',')}],'
      'testMs:$testMs,totalMs:$totalMs})';
}

/// In the inspect view, asks the page to run the migration [file] (an app
/// bundle's `migrate.js`): its default export is called with the page's
/// `db` and `{from, to}`, the run for at most [totalMs]. A [MigrationMsg]
/// answers. A later call replaces an earlier one; anywhere else the shell
/// answers with an error.
final class RunMigration extends HostCall {
  const RunMigration({
    this.file = 'migrate.js',
    required this.from,
    required this.to,
    this.totalMs = 30000,
    this.journal = false,
  });
  final String file;
  final int from;
  final int to;
  final int totalMs;

  /// Journal every write: each waits for a [JournalAck] of its [JournalMsg].
  final bool journal;
  @override
  String toJs() =>
      'window.claxHost?.runMigration({file:${jsString(file)},from:$from,'
      'to:$to,totalMs:$totalMs${journal ? ',journal:true' : ''}})';
}

/// The app's answer to [JournalMsg] [seq]: it keeps the entry ([ok]), so
/// the write goes ahead, or not, so it fails.
final class JournalAck extends HostCall {
  const JournalAck(this.seq, {required this.ok});
  final int seq;
  final bool ok;
  @override
  String toJs() => 'window.claxHost?.journalAck({seq:$seq,ok:$ok})';
}

/// In the migration view, puts back a journaled migration's [entries]
/// (`{path, before}`, in journal order) in reverse order: a
/// [JournalRestoredMsg] answers.
final class RestoreJournal extends HostCall {
  const RestoreJournal(this.entries);
  final List<Map<String, Object?>> entries;
  @override
  String toJs() =>
      'window.claxHost?.restoreJournal(JSON.parse(${jsString(jsonEncode({'entries': entries}))}))';
}

/// The app's answer to [DeviceRequest] [id]: [value] when [code] is null,
/// else the coded error ([code], [message]) the page's call rejects with.
final class DeviceResult extends HostCall {
  const DeviceResult.ok(this.id, this.value) : code = null, message = null;
  const DeviceResult.error(this.id, String this.code, String this.message)
    : value = null;

  final String id;
  final Object? value;
  final String? code;
  final String? message;

  @override
  String toJs() {
    final a = code == null
        ? {'id': id, 'ok': true, 'value': value}
        : {
            'id': id,
            'ok': false,
            'error': {'code': code, 'message': message},
          };
    return 'window.claxHost?.deviceResult(JSON.parse(${jsString(jsonEncode(a))}))';
  }
}

/// A running `location.watch`'s news for the page, under the page's
/// [watch] ID: an update ([value], `{lat, lon, accuracy, precise}`), or its
/// end ([code], [message]; `stopped` when the app left the screen), after
/// which nothing more comes for it.
final class DeviceEvent extends HostCall {
  const DeviceEvent.update(this.watch, Map<String, Object?> this.value)
    : code = null,
      message = null;
  const DeviceEvent.end(this.watch, String this.code, String this.message)
    : value = null;

  final String watch;
  final Map<String, Object?>? value;
  final String? code;
  final String? message;

  @override
  String toJs() {
    final a = code == null
        ? {'cap': 'location', 'watch': watch, 'kind': 'update', 'value': value}
        : {
            'cap': 'location',
            'watch': watch,
            'kind': 'end',
            'error': {'code': code, 'message': message},
          };
    return 'window.claxHost?.deviceEvent(JSON.parse(${jsString(jsonEncode(a))}))';
  }
}

/// A change for the home app's `home` capability: topic [name] (`apps`,
/// `makes`, `unread` or `key`) now holds [data], its whole latest value.
final class HomeDeviceEvent extends HostCall {
  const HomeDeviceEvent(this.name, this.data);

  final String name;
  final Object? data;

  @override
  String toJs() {
    final a = {'cap': 'home', 'name': name, 'data': data};
    return 'window.claxHost?.deviceEvent(JSON.parse(${jsString(jsonEncode(a))}))';
  }
}

/// Moves the page to fragment [hash] (`#…`), as a notification's tap asks.
final class OpenRoute extends HostCall {
  const OpenRoute(this.hash);
  final String hash;
  @override
  String toJs() => 'window.claxHost?.openRoute(${jsString(hash)})';
}

/// Whether an [AttestPointer] is a press, a release, or a touch the OS
/// took back.
enum PointerPhase { down, up, cancel }

/// A real finger pressed, lifted or cancelled on the WebView at ([x], [y])
/// logical pixels from its top left, on a screen of [devicePixelRatio].
///
/// The shell counts a press as the finger's arrival on the content frame
/// (`attestPointer`), which a long-press's pick needs when the WebView
/// sends no click after it; a cancel ends that press and counts as
/// nothing. The page takes CSS pixels of its layout
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
  ShellSession(
    this.base, {
    Random? random,
    this.native = const [],
    this.platform,
  }) : _random = random ?? Random.secure();

  /// The daemon's base URL (`http://127.0.0.1:<port>`).
  final Uri base;

  /// The overlays each [Init] declares native ([Init.native]).
  final List<String> native;

  /// The platform each [Init] names ([Init.platform]).
  final String? platform;
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

  /// The artifact ID of [url] when it is the shell's app view on [base]
  /// ([isShellUrl]); null otherwise.
  static String? artifactOf(Uri base, Uri? url) =>
      isShellUrl(base, url) ? url!.path.split('/')[2] : null;

  /// Whether calls may be made now.
  bool get canCall => _shell && _nonce != null;

  /// The artifact the loaded top-level document shows, from the URL the
  /// WebView committed it at; null while none has loaded or it is not the
  /// shell.
  String? get document => canCall ? _document : null;
  String? _document;

  /// A new top-level document started loading at [url]: nothing is
  /// accepted or called until it has loaded.
  void documentStarted(Uri? url) {
    _nonce = null;
    _document = null;
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
    _document = artifactOf(base, url);
    final bytes = List<int>.generate(16, (_) => _random.nextInt(256));
    final n = [for (final b in bytes) b.toRadixString(16).padLeft(2, '0')]
        .join();
    _nonce = n;
    return Init(n, visible: _visible, native: native, platform: platform);
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
