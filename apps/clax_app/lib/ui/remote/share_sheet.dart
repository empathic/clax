/// The Share sheet of a published app (spec 2026-10-21-app-push-pull §5.6):
/// the web Share dialog's model (`web/shell/src/ui/share-model.ts`, whose
/// words it uses), over the remote's sharing routes through [RemoteApi].
library;

import 'dart:async';

import 'package:flutter/foundation.dart' show defaultTargetPlatform;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:share_plus/share_plus.dart';

import '../../clax/api.dart' show ClaxApiError;
import '../../platform/device/mac_share.dart';
import '../../platform/share_anchor.dart';
import '../../remote/models.dart';
import '../../remote/remote_api.dart';
import '../../remote/remote_errors.dart';
import '../bundle_sheets.dart';
import '../kit/kit.dart';
import '../theme.dart';
import '../tokens.g.dart';

/// The words for each access (`ACCESS_LABEL`).
const accessLabel = {
  'viewer': 'Can view',
  'commenter': 'Can comment',
  'editor': 'Can edit',
};

/// The accesses anyone with the link may have (`LINK_ACCESSES`).
const linkAccesses = ['viewer', 'commenter'];

/// The words for each general access (`GENERAL_LABEL`).
const generalLabel = {
  'restricted': 'Restricted',
  'link': 'Anyone with the link',
  'public': 'Public',
};

/// The line under the general access (`GENERAL_NOTE`).
const generalNote = {
  'restricted': 'Only the people added can open it.',
  'link': 'Anyone with the link can open it, without signing in.',
  'public':
      // The web dialog's words, apostrophe included.
      "Anyone with the link can open it, and it is listed on this Clax's " // copy-check: allow
      'public page.',
};

/// The Comments switch's label (`COMMENTS_LABEL`).
const commentsLabel = 'Comments';

/// The line under the Comments switch, on and off (`COMMENTS_NOTE`).
const commentsNote = (
  on: 'People who can comment may comment.',
  off: 'Off: nobody can comment, whatever their access.',
);

/// What the add field says when what was typed names no one.
const addRefusal = 'Type a GitHub login, such as @octocat, or pick someone.';

/// [s] as a GitHub login (an `@` first is dropped), lowercased; null when
/// it is not one (`parseLogin`).
String? parseLogin(String s) {
  var l = s.trim();
  if (l.startsWith('@')) l = l.substring(1);
  return RegExp(r'^[A-Za-z0-9][A-Za-z0-9-]{0,38}$').hasMatch(l)
      ? l.toLowerCase()
      : null;
}

/// How a share is named: the person's name, else `@login` (`shareName`).
String shareName(RemoteShare s) =>
    s.name ?? (s.githubLogin != null ? '@${s.githubLogin}' : 'Someone');

/// The line under a share's name: its login, and that it waits for a
/// first sign-in (`shareDetail`).
String shareDetail(RemoteShare s) {
  if (s.pending) {
    return 'Not signed in yet: has access once they sign in with GitHub';
  }
  return s.githubLogin != null && s.name != null ? '@${s.githubLogin}' : '';
}

bool _holds(ShareCandidate c, List<RemoteShare> people) => people.any(
  (p) =>
      (p.person != null && p.person == c.person) ||
      (c.githubLogin != null && p.githubLogin == c.githubLogin!.toLowerCase()),
);

/// The suggestions for [query]: people not shared with yet whose name or
/// login starts a word with it, at most [max] (`suggest`).
List<ShareCandidate> suggest(
  String query,
  List<ShareCandidate> candidates,
  List<RemoteShare> people, {
  int max = 6,
}) {
  var q = query.trim().toLowerCase();
  if (q.startsWith('@')) q = q.substring(1);
  if (q.isEmpty) return const [];
  bool hit(String? s) {
    if (s == null) return false;
    final l = s.toLowerCase();
    return [...l.split(RegExp(r'[\s-]+')), l].any((w) => w.startsWith(q));
  }

  return candidates
      .where(
        (c) =>
            !_holds(c, people) &&
            (hit(c.name) || hit(c.githubLogin)),
      )
      .take(max)
      .toList();
}

/// Who the add field names (`subject`): a chosen suggestion's person;
/// else, for text with no `@` first that is exactly one of [candidates]'
/// names (any case), that person; else the login typed. Null when none,
/// or when the name is more than one candidate's: a word naming someone
/// here is never taken for a stranger's GitHub login, which takes the `@`.
ShareTarget? subject(
  String query,
  ShareCandidate? chosen,
  List<ShareCandidate> candidates,
) {
  if (chosen != null) return SharePerson(chosen.person);
  final typed = query.trim();
  if (!typed.startsWith('@')) {
    final named = [
      for (final c in candidates)
        if (c.name?.trim().toLowerCase() == typed.toLowerCase()) c,
    ];
    if (named.length == 1) return SharePerson(named.single.person);
    if (named.length > 1) return null;
  }
  final login = parseLogin(query);
  return login == null ? null : ShareLogin(login);
}

/// The link request general access [general] makes, at [access] for
/// anyone with the link (`linkRequest`).
({String? access, bool listed}) linkRequest(String general, String access) =>
    general == 'restricted'
    ? (access: null, listed: false)
    : (access: access, listed: general == 'public');

/// Puts [text] on the clipboard.
Future<void> copyToClipboard(String text) =>
    Clipboard.setData(ClipboardData(text: text));

/// Hands a link to a share sheet, pointing from `origin`.
typedef ShareLink = Future<void> Function(String url, String title, Rect origin);

/// Hands [url] to `share_plus`'s share sheet, pointing from [origin]:
/// everywhere but the Mac.
Future<void> shareLinkByPlugin(String url, String title, Rect origin) =>
    SharePlus.instance
        .share(
          ShareParams(text: url, subject: title, sharePositionOrigin: origin),
        )
        .then((_) {});

/// Hands [url] to the Mac's sharing picker ([MacShare]): `share_plus`'s
/// macOS plugin needs the engine's implicit view, which the multi-view Mac
/// app does not have, and would crash it.
Future<void> shareLinkByMac(String url, String title, Rect origin) =>
    const MacShare().text(url).then((_) {});

/// The share sheet for links on [platform]: [shareLinkByMac] on the Mac,
/// else [shareLinkByPlugin].
ShareLink shareLinkFor(TargetPlatform platform) =>
    platform == TargetPlatform.macOS ? shareLinkByMac : shareLinkByPlugin;

/// Shows public copy [raid]'s sharing on remote [rid] in a sheet: the
/// Comments switch, the add field, People with access, General access,
/// Copy link and the platform's share (once anyone but the owner can open
/// it) and Done. [host] names the remote in what the sheet says until its
/// sharing is read. [copyText] and [shareLink] replace the clipboard and
/// the platform's share sheet.
Future<void> showShareSheet(
  BuildContext context,
  RemoteApi remote, {
  required String rid,
  required String raid,
  required String title,
  String? host,
  Future<void> Function(String text)? copyText,
  Future<void> Function(String url, String title, Rect origin)? shareLink,
}) => showBundleSheet<void>(
  context,
  ShareSheet(
    remote: remote,
    rid: rid,
    raid: raid,
    title: title,
    host: host,
    copyText: copyText ?? copyToClipboard,
    shareLink: shareLink,
  ),
);

/// The sheet [showShareSheet] shows. Every change goes to the remote, and
/// the sheet then reads the sharing again, after a failure too: what it
/// shows is what the remote holds, never a change it only hoped for.
class ShareSheet extends StatefulWidget {
  const ShareSheet({
    super.key,
    required this.remote,
    required this.rid,
    required this.raid,
    required this.title,
    this.host,
    this.copyText = copyToClipboard,
    this.shareLink,
  });

  final RemoteApi remote;
  final String rid;
  final String raid;
  final String title;
  final String? host;
  final Future<void> Function(String text) copyText;
  /// The share sheet for the link; null is the platform's
  /// ([shareLinkFor]).
  final ShareLink? shareLink;

  @override
  State<ShareSheet> createState() => _ShareSheetState();
}

class _ShareSheetState extends State<ShareSheet> {
  final _who = TextEditingController();
  Sharing? _data;
  Object? _loadError;
  var _busy = false;
  var _err = '';
  var _copied = false;
  ShareCandidate? _chosen;
  var _addAccess = 'viewer';
  Timer? _timer;

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  @override
  void dispose() {
    _timer?.cancel();
    _who.dispose();
    super.dispose();
  }

  String get _host {
    final u = _data?.url;
    final h = u == null ? '' : Uri.tryParse(u)?.host ?? '';
    return h.isNotEmpty ? h : widget.host ?? 'the remote';
  }

  Future<void> _load() async {
    setState(() => _loadError = null);
    try {
      final s = await widget.remote.sharing(widget.rid, widget.raid);
      if (mounted) setState(() => _data = s);
    } on Object catch (e) {
      debugPrint('clax remote: the sharing of ${widget.raid} was not read: $e');
      if (mounted) setState(() => _loadError = e);
    }
  }

  /// Why a change failed, for [e]; [reading] when it was the sharing's
  /// first read that failed rather than a change.
  String _failText(Object e, {bool reading = false}) {
    final h = _host;
    final otherwise = reading
        ? 'Couldn’t read who can open ${widget.title}.'
        : 'Couldn’t change who can open ${widget.title}.';
    if (e is ClaxApiError) {
      if (remoteLine(e, host: h, action: 'share apps') case final line?) {
        return line;
      }
      if (e.status == 403) {
        return 'Only the owner and the maker of ${widget.title} can share '
            'it.';
      }
      if (e.status == 404) return '${widget.title} is no longer on $h.';
      // The remote's refusal says what to fix (already_owner,
      // invalid_login, unknown_person), as the web dialog shows it.
      if (e.status == 400 && e.message.trim().isNotEmpty) return e.message;
    }
    return otherwise;
  }

  /// Runs [change], then reads the sharing again whether it landed or
  /// not: a change whose answer was lost may have landed.
  Future<bool> _act(Future<void> Function() change) async {
    if (_busy) return false;
    setState(() {
      _busy = true;
      _err = '';
    });
    var ok = true;
    var failure = '';
    try {
      await change();
    } on Object catch (e) {
      debugPrint('clax remote: a sharing change failed: $e');
      ok = false;
      failure = _failText(e);
    }
    Sharing? fresh;
    try {
      fresh = await widget.remote.sharing(widget.rid, widget.raid);
    } on Object catch (e) {
      debugPrint('clax remote: the sharing was not read again: $e');
      failure = ok
          ? '${_failText(e)} What this shows may be out of date.'
          : '$failure What this shows may be out of date.';
      ok = false;
    }
    if (!mounted) return ok;
    setState(() {
      if (fresh != null) _data = fresh;
      _err = failure;
      _busy = false;
    });
    return ok;
  }

  Future<void> _add() async {
    final d = _data;
    if (d == null) return;
    final to = subject(_who.text, _chosen, d.suggestions);
    if (to == null) {
      setState(() => _err = addRefusal);
      return;
    }
    // Only the text sent is cleared: what is typed while the add is in
    // flight stays.
    final sent = _who.text;
    final access = _addAccess;
    if (await _act(
          () => widget.remote.share(widget.rid, widget.raid, to, access),
        ) &&
        mounted &&
        _who.text == sent) {
      setState(() {
        _who.clear();
        _chosen = null;
      });
    }
  }

  Future<void> _setGeneral(String general, String access) {
    final r = linkRequest(general, access);
    return _act(
      () => widget.remote.setLink(
        widget.rid,
        widget.raid,
        r.access,
        listed: r.listed,
      ),
    );
  }

  Future<void> _copy(String url) async {
    try {
      await widget.copyText(url);
    } on Object catch (e) {
      debugPrint('clax remote: the link was not copied: $e');
      return;
    }
    if (!mounted) return;
    setState(() => _copied = true);
    _timer?.cancel();
    _timer = Timer(const Duration(milliseconds: 1600), () {
      if (mounted) setState(() => _copied = false);
    });
  }

  @override
  Widget build(BuildContext context) {
    final d = _data;
    final open =
        d != null &&
        d.canShare &&
        d.url != null &&
        (d.general != 'restricted' || d.people.isNotEmpty);
    return BundleSheetFrame(
      face: null,
      title: 'Share ${widget.title}',
      body: [
        if (d == null && _loadError == null)
          const Padding(
            padding: EdgeInsets.all(ClaxSpace.s16),
            child: Center(child: CircularProgressIndicator.adaptive()),
          )
        else if (d == null)
          ..._unread(context)
        else if (!d.canShare)
          _Line(
            'Only the owner and the maker of ${widget.title} can share it.',
          )
        else
          ..._form(context, d),
        if (_err.isNotEmpty)
          Padding(
            padding: const EdgeInsets.only(top: ClaxSpace.s12),
            child: Semantics(
              liveRegion: true,
              child: Text(
                _err,
                key: const ValueKey('share-error'),
                style: Theme.of(context).textTheme.bodyMedium?.copyWith(
                  color: claxPalette(context).danger,
                ),
              ),
            ),
          ),
      ],
      actions: [
        if (d != null && d.canShare) ...[
          ClaxButton(
            key: const ValueKey('share-copy'),
            icon: Icons.link_rounded,
            label: _copied ? 'Copied' : 'Copy link',
            onPressed: open ? () => unawaited(_copy(d.url!)) : null,
          ),
          Builder(
            builder: (button) => ClaxButton(
              key: const ValueKey('share-send'),
              icon: Icons.ios_share_rounded,
              label: 'Share link…',
              onPressed: open
                  ? () => unawaited(
                      (widget.shareLink ?? shareLinkFor(defaultTargetPlatform))(
                        d.url!,
                        widget.title,
                        shareAnchorOf(button),
                      ),
                    )
                  : null,
            ),
          ),
        ],
        ClaxButton(
          key: const ValueKey('share-done'),
          kind: ClaxButtonKind.primary,
          isDefault: true,
          label: 'Done',
          onPressed: () => Navigator.of(context).pop(),
        ),
      ],
    );
  }

  List<Widget> _unread(BuildContext context) => [
    _Line(_failText(_loadError!, reading: true)),
    const SizedBox(height: ClaxSpace.s8),
    Align(
      alignment: Alignment.centerLeft,
      child: ClaxButton(
        key: const ValueKey('share-retry'),
        label: 'Try again',
        onPressed: () => unawaited(_load()),
      ),
    ),
  ];

  List<Widget> _form(BuildContext context, Sharing d) {
    final theme = Theme.of(context);
    final hints = _chosen != null
        ? const <ShareCandidate>[]
        : suggest(_who.text, d.suggestions, d.people);
    final general = d.general ?? 'restricted';
    final link = d.linkAccess ?? 'viewer';
    final generals = [
      for (final g in generalLabel.keys)
        if (g != 'public' || d.publicGallery || general == 'public') g,
    ];
    Widget heading(String text) => Padding(
      padding: const EdgeInsets.only(top: ClaxSpace.s16, bottom: 4),
      child: Text(text, style: theme.textTheme.titleSmall),
    );
    return [
      ClaxSwitch(
        key: const ValueKey('share-comments'),
        label: commentsLabel,
        value: d.comments,
        onChanged: _busy
            ? null
            : (on) => unawaited(
                _act(
                  () => widget.remote.setComments(widget.rid, widget.raid, on),
                ),
              ),
      ),
      _Line(d.comments ? commentsNote.on : commentsNote.off),
      const SizedBox(height: ClaxSpace.s12),
      ClaxTextField(
        key: const ValueKey('share-who'),
        controller: _who,
        label: 'GitHub login',
        showLabel: false,
        hint: d.suggestions.isNotEmpty
            ? '@github-login or a name'
            : '@github-login',
        autocorrect: false,
        keyboardType: TextInputType.emailAddress,
        onChanged: (_) => setState(() {
          _chosen = null;
          _err = '';
        }),
        onSubmitted: (_) => unawaited(_add()),
      ),
      for (final c in hints)
        Align(
          alignment: Alignment.centerLeft,
          child: TextButton(
            key: ValueKey('share-hint-${c.person}'),
            onPressed: () => setState(() {
              _chosen = c;
              _who.text = c.name ?? '@${c.githubLogin ?? ''}';
            }),
            child: Text(
              c.name != null && c.githubLogin != null
                  ? '${c.name}  @${c.githubLogin}'
                  : c.name ?? '@${c.githubLogin ?? ''}',
            ),
          ),
        ),
      const SizedBox(height: ClaxSpace.s8),
      // A Wrap, so at large text on a narrow phone Add goes under the
      // picker rather than past the sheet's edge.
      Wrap(
        alignment: WrapAlignment.spaceBetween,
        crossAxisAlignment: WrapCrossAlignment.center,
        runSpacing: ClaxSpace.s8,
        children: [
          _Choice(
            key: const ValueKey('share-add-access'),
            semanticLabel: 'Access for the person you add',
            label: accessLabel[_addAccess]!,
            items: [
              for (final MapEntry(:key, :value) in accessLabel.entries)
                ClaxMenuItem(key, value, checked: key == _addAccess),
            ],
            onPicked: _busy ? null : (a) => setState(() => _addAccess = a),
          ),
          ClaxButton(
            key: const ValueKey('share-add'),
            kind: ClaxButtonKind.tonal,
            label: 'Add',
            onPressed: _busy ? null : () => unawaited(_add()),
          ),
        ],
      ),
      const _Line(
        'They get access when they sign in with GitHub. Can edit also '
        'publishes new versions and resolves threads; only the owner and '
        'the maker share or delete.',
      ),
      heading('People with access'),
      if (d.people.isEmpty)
        const _Line('No one yet. The owner and the maker can always open it.')
      else
        for (final s in d.people) _person(context, s),
      heading('General access'),
      Wrap(
        spacing: ClaxSpace.s8,
        runSpacing: ClaxSpace.s8,
        children: [
          _Choice(
            key: const ValueKey('share-general'),
            semanticLabel: 'General access',
            label: generalLabel[general] ?? general,
            items: [
              for (final g in generals)
                ClaxMenuItem(g, generalLabel[g]!, checked: g == general),
            ],
            onPicked: _busy ? null : (g) => unawaited(_setGeneral(g, link)),
          ),
          if (general != 'restricted')
            _Choice(
              key: const ValueKey('share-link-access'),
              semanticLabel: 'Access for anyone with the link',
              label: accessLabel[link]!,
              items: [
                for (final a in linkAccesses)
                  ClaxMenuItem(a, accessLabel[a]!, checked: a == link),
              ],
              onPicked: _busy
                  ? null
                  : (a) => unawaited(_setGeneral(general, a)),
            ),
        ],
      ),
      _Line(generalNote[general] ?? ''),
    ];
  }

  Widget _person(BuildContext context, RemoteShare s) {
    final theme = Theme.of(context);
    final p = claxPalette(context);
    final name = shareName(s);
    final detail = shareDetail(s);
    // As the web dialog: by person once known, else by login.
    final ShareTarget to = s.person != null
        ? SharePerson(s.person!)
        : ShareLogin(s.githubLogin ?? '');
    final choice = _Choice(
      key: ValueKey('share-access-${s.id}'),
      label: accessLabel[s.access] ?? s.access,
      semanticLabel: 'Access for $name',
      items: [
        for (final MapEntry(:key, :value) in accessLabel.entries)
          ClaxMenuItem(
            key,
            value,
            checked: key == s.access,
          ),
        const ClaxMenuItem.separator(),
        const ClaxMenuItem('remove', 'Remove', destructive: true),
      ],
      onPicked: _busy
          ? null
          : (v) => unawaited(
              v == 'remove'
                  ? _act(
                      () => widget.remote.unshare(
                        widget.rid,
                        widget.raid,
                        s.id,
                      ),
                    )
                  : v == s.access
                  ? Future<void>.value()
                  : _act(
                      () => widget.remote.share(
                        widget.rid,
                        widget.raid,
                        to,
                        v,
                      ),
                    ),
            ),
    );
    // At large text the name keeps the row's width and the picker goes
    // under it, so a long login is not broken mid-word beside it.
    final stacked = MediaQuery.textScalerOf(context).scale(16) > 24;
    final who = Row(
      children: [
        ExcludeSemantics(
          child: CircleAvatar(
            radius: 14,
            backgroundColor: p.border,
            child: Text(
              name.replaceFirst('@', '').characters.take(1).toString().toUpperCase(),
              style: theme.textTheme.labelMedium?.copyWith(color: p.muted),
            ),
          ),
        ),
        const SizedBox(width: ClaxSpace.s8),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(name, style: theme.textTheme.bodyMedium),
              if (detail.isNotEmpty)
                Text(
                  detail,
                  style: theme.textTheme.bodySmall?.copyWith(
                    color: p.muted,
                  ),
                ),
            ],
          ),
        ),
        if (!stacked) choice,
      ],
    );
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: ClaxSpace.s4),
      child: stacked
          ? Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                who,
                Padding(
                  padding: const EdgeInsetsDirectional.only(start: 28),
                  child: choice,
                ),
              ],
            )
          : who,
    );
  }
}

/// A button naming the current choice, which opens a menu of [items].
class _Choice extends StatelessWidget {
  const _Choice({
    super.key,
    required this.label,
    required this.items,
    required this.onPicked,
    this.semanticLabel,
  });

  final String label;
  final List<ClaxMenuItem> items;
  final ValueChanged<String>? onPicked;
  final String? semanticLabel;

  @override
  Widget build(BuildContext context) {
    final picked = onPicked;
    final button = ClaxButton(
      icon: Icons.unfold_more_rounded,
      label: label,
      onPressed: picked == null
          ? null
          : () async {
              final v = await ClaxMenu.show(
                context,
                items: items,
                anchor: shareAnchorOf(context),
              );
              if (v != null) picked(v);
            },
    );
    return semanticLabel == null
        ? button
        : Semantics(label: semanticLabel, child: button);
  }
}

class _Line extends StatelessWidget {
  const _Line(this.text);
  final String text;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(top: 4),
    child: Text(
      text,
      style: Theme.of(context).textTheme.bodySmall?.copyWith(
        color: claxPalette(context).muted,
      ),
    ),
  );
}
