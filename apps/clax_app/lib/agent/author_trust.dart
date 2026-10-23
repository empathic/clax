/// The person's trust in the author of an app someone else wrote, asked
/// once before Claude first reads that app to change or combine it.
library;

import 'package:flutter/widgets.dart';

import '../bundle/lineage_store.dart';
import '../platform/bundle_key.dart' show keyFingerprint;
import '../platform/settings_store.dart';
import '../remote/models.dart' show PublishedBy, Remote, VersionSource;
import '../ui/author_trust_sheet.dart';

/// Whether [by], publishing on the hosted Clax at [origin], is the account
/// one of [remotes] is signed in as there: the same public ID. A name or
/// login is the account's to choose, so neither counts, and a publisher
/// without a public ID is never the person's own.
bool isOwnAccount(Iterable<Remote> remotes, String origin, PublishedBy by) {
  final id = by.publicId;
  return id != null &&
      remotes.any(
        (r) => r.connected && r.origin == origin && r.personPublicId == id,
      );
}

/// Who wrote an app's code, when it is someone other than this install.
typedef AppAuthor = ({String key, String who, String title});

/// Asks once per app and author, before a run reads an app whose code
/// someone else wrote, whether the person trusts that author
/// ([showAuthorTrustSheet]), and keeps a Continue in
/// [AppSettings.trustedAuthors].
///
/// An app's author is its lineage's ([LineageStore]): an app installed or
/// got from someone else is its signer's; a fork made here of such an app
/// is its original's ([LineageEntry.upstream]). An app made here, or
/// signed by [ownKey]'s key, has no other author and is never asked about.
///
/// A merge also asks about the author of the version it brings in
/// ([ensureSource]), unless that version is the person's own: signed by
/// [ownKey]'s key, or published by their account ([ownAccount]).
class AuthorTrust {
  AuthorTrust({
    required this.lineages,
    required this.settings,
    this.ownKey,
    this.ownAccount,
  });

  final LineageStore lineages;

  /// Where the answers are kept.
  final SettingsStore settings;

  /// This install's author key, base64url, or null when it has none.
  final Future<String?> Function()? ownKey;

  /// Whether [by], publishing on the hosted Clax at `origin`, is the
  /// person's own account there; null knows of no account.
  final Future<bool> Function(String origin, PublishedBy by)? ownAccount;

  /// The answer of the sheet showing for each app and author, which every
  /// run that asks the same meanwhile shares.
  final _asking = <(String, String), Future<bool>>{};

  /// The author of an app whose lineage Clax cannot read: its entry does
  /// not parse, or the lineages were damaged and set aside, so who wrote
  /// it is not known and the person is asked as about a stranger.
  static const unknown = (
    key: 'unknown',
    who: 'an unknown author',
    title: 'This app',
  );

  /// App [aid]'s author when someone other than this install wrote its
  /// code, or null. An app with no lineage entry is one made here, unless
  /// its entry is there but unreadable or the lineages were lost
  /// ([LineageStore.lost]): then its author is [unknown], never taken for
  /// the person's own.
  Future<AppAuthor?> authorOf(String aid) async {
    final all = await lineages.all();
    final e = all[aid];
    if (e == null) {
      if (await lineages.unreadable(aid) || lineages.lost) return unknown;
      return null;
    }
    final String key;
    String? name;
    switch (e) {
      case LineageEntry(role: LineageRole.installed):
        key = e.key;
        name = e.name;
      case LineageEntry(upstream: final up?):
        key = up.key;
        name = [
          for (final o in all.values)
            if (o.key == key && o.name != null) o.name!,
        ].firstOrNull;
      default:
        return null;
    }
    if (key == await ownKey?.call()) return null;
    return (key: key, who: name ?? keyFingerprint(key) ?? key, title: e.title);
  }

  /// Whether a run may read app [aid]: true at once when it has no other
  /// author ([authorOf]) or the person trusted its current one; otherwise
  /// the sheet asks over [context], and Continue is kept for this app and
  /// author. Cancel, or closing the sheet, answers false and keeps
  /// nothing, so the next run asks again. A null or unmounted [context]
  /// answers false.
  ///
  /// The publisher of the app's live version is asked about too, as
  /// [ensureSource] asks ([LineageEntry.liveFrom]): an update or a public
  /// version taken into an app made here brings in someone else's code
  /// without a merge.
  ///
  /// An app got from the person's own account (another of their devices
  /// signed it, and [LineageEntry.liveFrom] names their account as its
  /// publisher) is theirs and asks nothing.
  Future<bool> ensure(BuildContext? context, String aid) async {
    final author = await authorOf(aid);
    final e = await lineages.entry(aid);
    final from = e?.liveFrom;
    final ownCopy =
        author != null &&
        from != null &&
        from.key == author.key &&
        await _isOwn(from);
    if (author != null && !ownCopy && !await _kept(aid, author.key)) {
      if (context == null || !context.mounted) return false;
      if (!await _asked(context, aid, author)) return false;
    }
    if (from == null || await _isOwn(from) || await _kept(aid, from.key)) {
      return true;
    }
    if (context == null || !context.mounted) return false;
    return _asked(context, aid, (
      key: from.key,
      who: from.who,
      title: e!.title,
    ));
  }

  /// Whether a merge into app [aid] ([title]) may read the version
  /// [source] made: true at once when it is the person's own (signed by
  /// [ownKey]'s key, or published by [ownAccount]'s account) or they
  /// trusted [source] for this app; otherwise asked as [ensure] asks,
  /// naming [source]'s author, and a Continue is kept for this app and
  /// that author.
  Future<bool> ensureSource(
    BuildContext? context,
    String aid,
    VersionSource source, {
    required String title,
  }) async {
    if (await _isOwn(source) || await _kept(aid, source.key)) return true;
    if (context == null || !context.mounted) return false;
    return _asked(context, aid, (
      key: source.key,
      who: source.who,
      title: title,
    ));
  }

  /// Whether [source] is the person's own: signed with [ownKey]'s key, or
  /// published by their account ([ownAccount]).
  Future<bool> _isOwn(VersionSource source) async {
    if (source.key == await ownKey?.call()) return true;
    if ((source.origin, source.account, ownAccount) case (
      final origin?,
      final by?,
      final own?,
    )) {
      return own(origin, by);
    }
    return false;
  }

  /// Whether the person kept a Continue for app [aid] and author [key].
  Future<bool> _kept(String aid, String key) async =>
      ((await settings.load()).trustedAuthors[aid] ?? const []).contains(key);

  /// [_ask] once at a time for app [aid] and author [a]: a second run
  /// asking meanwhile shares the sheet on screen.
  Future<bool> _asked(BuildContext context, String aid, AppAuthor a) {
    // The callback returns nothing: whenComplete waits on a returned
    // future, and the one removed is this very answer.
    return _asking[(aid, a.key)] ??= _ask(context, aid, a).whenComplete(() {
      _asking.remove((aid, a.key));
    });
  }

  Future<bool> _ask(BuildContext context, String aid, AppAuthor a) async {
    final trusted =
        await showAuthorTrustSheet(context, app: a.title, who: a.who) ?? false;
    if (trusted) {
      final s = await settings.load();
      (s.trustedAuthors[aid] ??= []).add(a.key);
      await settings.save(s);
    }
    return trusted;
  }
}
