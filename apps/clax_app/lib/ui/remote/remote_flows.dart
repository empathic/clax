/// The flows of publishing apps to a hosted Clax and getting them from it
/// (spec 2026-10-21-app-push-pull §5.3, §5.5): the sheets, over the
/// services that do the work.
library;

import 'dart:async';

import 'package:flutter/material.dart';

import '../../bundle/exporter.dart';
import '../../bundle/installer.dart' as svc show OpenedBundle;
import '../../bundle/lineage_store.dart';
import '../../bundle/trust.dart';
import '../../bundle/updater.dart';
import '../../clax/api.dart' show ClaxApiError;
import '../../remote/link_remote.dart';
import '../../remote/merge.dart';
import '../../remote/models.dart';
import '../../remote/publisher.dart';
import '../../remote/puller.dart';
import '../../remote/remote_api.dart';
import '../../remote/remote_errors.dart';
import '../bundle_sheets.dart';
import '../developer.dart';
import '../kit/kit.dart';
import '../user_message.dart';
import 'diverged_sheet.dart';
import 'library_screen.dart';
import 'publish_sheet.dart';
import 'published_sheet.dart';
import 'share_sheet.dart';

/// Publish to clax.dev…, Get my apps and Unpublish, over [publisher],
/// [puller] and [remote].
///
/// Publishing and getting need a remote this device is signed in to; with
/// none, [connect] runs the sign-in (Settings › Sharing's), or, without
/// it, a sheet says to connect first. Share… on a published app runs
/// [shareCopy] (the sharing sheet), else opens the copy's page, whose own
/// sharing is the same; Open on clax.dev opens it with [openUrl].
class RemoteFlows {
  RemoteFlows({
    required this.remote,
    required this.publisher,
    required this.puller,
    required this.exporter,
    required this.store,
    required this.open,
    this.bundles,
    this.openUrl,
    this.connect,
    this.shareCopy,
    this.unlinked,
    this.merger,
  });

  /// Combines versions: a diverged app's choices (§5.10); null offers
  /// none of them.
  final Merger? merger;

  /// App `aid` was unlinked here (Unpublish): what waited on its public
  /// copy (held replies) goes.
  final void Function(String aid)? unlinked;

  final RemoteApi remote;
  final Publisher publisher;
  final Puller puller;
  final Exporter exporter;
  final LineageStore store;

  /// Opens local app [aid].
  final void Function(String aid) open;

  /// The bundle flows, for an update a pull finds waiting.
  final BundleFlows? bundles;

  /// Opens [url] in the browser.
  final Future<bool> Function(Uri url)? openUrl;

  /// Signs in to a remote, answering the remote once connected.
  final Future<Remote?> Function(BuildContext context)? connect;

  /// Shows public copy [raid]'s sharing on [rid]; null shows
  /// [showShareSheet].
  final Future<void> Function(
    BuildContext context, {
    required String rid,
    required String raid,
    required String title,
  })?
  shareCopy;

  String? _host;

  /// The name of the remote this device publishes to: the connected one
  /// last seen, else clax.dev.
  String get host => _host ?? 'clax.dev';

  /// The progress sheet's step while a publish waits for a connection.
  static const waitingStep =
      'Waiting for a connection. Clax keeps trying after you close this.';

  /// The menu item that publishes an app.
  String get publishLabel => 'Publish to $host…';

  /// The remote this device is signed in to at [origin] (a linked app's),
  /// or the first when [origin] is null; else the one [connect] signs in
  /// to, when it is at [origin].
  Future<Remote?> _connected(BuildContext context, {String? origin}) async {
    List<Remote> all;
    try {
      all = await remote.remotes();
    } on ClaxApiError {
      all = const [];
    }
    bool at(Remote r) =>
        r.connected && (origin == null || originOf(r.origin) == originOf(origin));
    final on = [
      for (final r in all)
        if (at(r)) r,
    ];
    if (on.isNotEmpty) {
      _host = on.first.name;
      return on.first;
    }
    if (!context.mounted) return null;
    final c = connect;
    if (c != null) {
      final r = await c(context);
      if (r == null || !at(r)) return null;
      _host = r.name;
      await publisher.links.relinkAll(r);
      unawaited(publisher.resume());
      return r;
    }
    await showBundleSheet<bool>(
      context,
      BundleNoticeSheet(
        title: 'Not connected',
        message: 'Connect to $host in Settings › Sharing first.',
      ),
    );
    return null;
  }

  /// The menu item that publishes app [entry]: Publish fork for an app
  /// installed from someone else's (§5.12), else [publishLabel].
  String publishLabelOf(LineageEntry? entry) =>
      (entry?.publishesAsFork ?? false) ? 'Publish fork…' : publishLabel;

  /// Publishes local [version] of [aid] (its current version when null):
  /// the publish sheet on its first publish, then the work under a
  /// progress sheet ("Waiting for a connection" while offline), then the
  /// published sheet.
  ///
  /// An app installed from someone else's goes as Publish fork (spec
  /// 2026-10-21-app-push-pull §5.12, [Publisher.publishFork]): the publish
  /// sheet asks for its name, prefilled "<title> (yours)", and it goes up
  /// as a new app of the person's own, Restricted with comments off; a copy
  /// linked to the original is relinked to it.
  Future<void> publish(
    BuildContext context,
    String aid, {
    int? version,
    String? svg,
  }) async {
    final entry = await store.entry(aid);
    final fork = entry?.publishesAsFork ?? false;
    final link = fork ? null : entry?.link;
    if (!context.mounted) return;
    final r = await _connected(context, origin: entry?.link?.remoteOrigin);
    if (r == null || !context.mounted) return;
    final ExportPlan plan;
    final _Versions d;
    try {
      plan = await exporter.plan(aid);
      d = await _detail(aid);
    } on Object catch (e) {
      if (!context.mounted) return;
      await _failed(context, 'Couldn’t read this app to publish it.', e);
      return;
    }
    if (!context.mounted) return;
    final n = version ?? d.current;
    var includeData = false;
    bool? auto;
    String? name;
    if (link == null) {
      final files = d.files[n] ?? (count: plan.fileCount, bytes: plan.fileBytes);
      final choice = await showBundleSheet<PublishChoice>(
        context,
        PublishSheet(
          title: plan.title,
          host: r.name,
          login: r.personName,
          version: n,
          fileCount: files.count,
          fileBytes: files.bytes,
          dataAllowed: plan.dataAllowed,
          forkName: fork ? '${plan.title} (yours)' : null,
          svg: svg,
          word: plan.iconWord,
          seed: aid,
        ),
      );
      if (choice == null || !context.mounted) return;
      includeData = choice.includeData;
      auto = choice.autoPublish;
      name = choice.name;
    }
    final shownTitle = name ?? plan.title;
    final progress = ValueNotifier<BundleProgress>(
      BundleWorking('Publishing $shownTitle…'),
    );
    void onWaiting() {
      if (publisher.waiting.value.contains(aid)) {
        progress.value = BundleWorking(
          'Publishing $shownTitle…',
          waitingStep,
        );
      }
    }

    publisher.waiting.addListener(onWaiting);
    final nav = Navigator.of(context);
    var up = true;
    // The publish goes on without the sheet: closed, it keeps trying in
    // the background (Publisher.waiting) and says only how it ended badly.
    final shown = showBundleSheet<bool>(
      context,
      BundleProgressSheet(progress: progress, closable: true),
    ).whenComplete(() => up = false);
    final PublishOutcome out;
    try {
      out = name != null
          ? await publisher.publishFork(
              aid,
              name: name,
              version: n,
              includeData: includeData,
              rid: r.id,
              autoPublish: auto ?? false,
            )
          : await publisher.publish(
              aid,
              version: n,
              includeData: includeData,
              rid: r.id,
              autoPublish: auto,
            );
    } finally {
      publisher.waiting.removeListener(onWaiting);
    }
    Future<void> failed(String message, [List<String> raw = const []]) async {
      final details = context.mounted ? _developer(context, raw) : <String>[];
      if (up) {
        progress.value = BundleFailed(message, details: details);
        await shown;
      } else if (context.mounted) {
        await showBundleSheet<bool>(
          context,
          BundleProgressSheet(
            progress: ValueNotifier(BundleFailed(message, details: details)),
          ),
        );
      }
    }

    switch (out) {
      case Published(:final url, :final remoteVersion):
        if (!up) return;
        nav.pop();
        await shown;
        if (!context.mounted) return;
        final picked = await showBundleSheet<PublishedAction>(
          context,
          PublishedSheet(
            title: shownTitle,
            host: r.name,
            version: remoteVersion,
            url: url,
            restricted: link == null,
            svg: svg,
            word: plan.iconWord,
            seed: aid,
          ),
        );
        if (!context.mounted) return;
        switch (picked) {
          case PublishedAction.share:
            await share(context, aid);
          case PublishedAction.open:
            await openUrl?.call(Uri.parse(url));
          case null:
            break;
        }
      case Moved(:final remoteVersion):
        // Both moved: the diverged sheet's choices, in place of the
        // publish's sheet.
        if (merger != null && up) {
          nav.pop();
          await shown;
          if (context.mounted) await diverged(context, aid);
          return;
        }
        await failed(
          '${plan.title} changed on ${r.name} since this device last '
          'published it (it is at v$remoteVersion there), so nothing was '
          'published.',
        );
      case Refused(:final message, :final details):
        await failed(message, details);
    }
  }

  /// Diverged app [aid] (§5.10): the diverged sheet over its public head,
  /// then the choice under a progress sheet: Combine (offered as "Your
  /// version with vN" before it is published), Take theirs, or Publish
  /// as a new app.
  Future<void> diverged(BuildContext context, String aid) async {
    final m = merger;
    final link = await _resolved(aid);
    if (m == null || link == null || !context.mounted) return;
    final host = Uri.parse(link.remoteOrigin).host;
    final RemoteHead head;
    try {
      head = await headOf(link);
    } on ClaxApiError catch (e) {
      if (context.mounted) {
        await _failed(context, 'Couldn’t reach $host.', e);
      }
      return;
    }
    final title = (await store.entry(aid))?.title ?? head.title;
    if (!context.mounted) return;
    final choice = await showBundleSheet<DivergeChoice>(
      context,
      DivergedSheet(
        title: title,
        host: host,
        who: head.publishedBy.handle,
        headVersion: head.version,
        baseVersion: link.baseRemoteVersion,
        note: head.note,
        canCombine: m.canMerge,
        seed: aid,
      ),
    );
    if (choice == null || !context.mounted) return;
    await _resolve(context, aid, choice, title: title, host: host, head: head);
  }

  /// Publish as a new app for linked app [aid] (Kept mine's, §5.4; or
  /// Diverged's): a new public copy, the app relinked to it.
  ///
  /// A recipient's copy goes as Publish fork ([publish]).
  Future<void> publishAsNewApp(BuildContext context, String aid) async {
    if ((await store.entry(aid))?.publishesAsFork ?? false) {
      if (context.mounted) await publish(context, aid);
      return;
    }
    if (!context.mounted) return;
    final m = merger;
    final link = await _resolved(aid);
    if (m == null || link == null || !context.mounted) return;
    final title = (await store.entry(aid))?.title ?? '';
    if (!context.mounted) return;
    await _resolve(
      context,
      aid,
      DivergeChoice.newApp,
      title: title,
      host: Uri.parse(link.remoteOrigin).host,
    );
  }

  /// The update waiting for linked app [aid] (§5.7): a version made on
  /// the web for an author (§5.10's Take theirs, after its sheet), else
  /// the newest bundle fetched and checked against the head, then the
  /// update sheet.
  Future<void> update(BuildContext context, String aid) async {
    final link = await _resolved(aid);
    if (link == null || !context.mounted) return;
    final m = merger;
    if (link.isAuthor && m != null) {
      RemoteHead? head;
      try {
        head = await headOf(link);
      } on ClaxApiError catch (e) {
        debugPrint('clax remote: the head of ${link.url}: ${e.message}');
      }
      if (head != null &&
          head.webMade &&
          head.version > link.baseRemoteVersion) {
        final host = Uri.parse(link.remoteOrigin).host;
        final title = (await store.entry(aid))?.title ?? head.title;
        if (!context.mounted) return;
        final go = await showBundleSheet<bool>(
          context,
          WebVersionSheet(
            title: title,
            host: host,
            who: head.publishedBy.handle,
            version: head.version,
            seed: aid,
          ),
        );
        if (go != true || !context.mounted) return;
        await _resolve(
          context,
          aid,
          DivergeChoice.takeTheirs,
          title: title,
          host: host,
          head: head,
        );
        return;
      }
    }
    final out = await puller.pull(
      Uri.parse(link.url),
      confirm: (_) async => false,
    );
    if (!context.mounted) return;
    switch (out) {
      case PullUpdate(:final opened):
        await bundles?.offerUpdate(context, opened);
      case PullRefused(:final message):
        await showBundleSheet<bool>(
          context,
          BundleNoticeSheet(title: 'Couldn’t get the update', message: message),
        );
      case PullHave():
        await showBundleSheet<bool>(
          context,
          const BundleNoticeSheet(
            title: 'No update waiting',
            message: 'This app has the newest version there is.',
          ),
        );
      case PullInstalled() || PullCancelled():
        break;
    }
  }

  Future<void> _resolve(
    BuildContext context,
    String aid,
    DivergeChoice choice, {
    required String title,
    required String host,
    RemoteHead? head,
  }) async {
    final m = merger!;
    final n = head?.version;
    final headline = switch (choice) {
      DivergeChoice.combine => 'Combining v$n with yours…',
      DivergeChoice.takeTheirs => 'Taking v$n from $host…',
      DivergeChoice.newApp => 'Publishing $title as a new app…',
    };
    final progress = ValueNotifier<BundleProgress>(
      BundleWorking(
        headline,
        choice == DivergeChoice.combine ? 'Claude is combining them' : null,
        choice != DivergeChoice.newApp,
      ),
    );
    final cancel = Completer<void>();
    final nav = Navigator.of(context);
    var up = true;
    final newApp = choice == DivergeChoice.newApp;
    // A new app's publish goes on without the sheet, as [publish]'s does:
    // it can be closed, and says when it waits for a connection.
    void onWaiting() {
      if (publisher.waiting.value.contains(aid)) {
        progress.value = BundleWorking(headline, waitingStep);
      }
    }

    if (newApp) publisher.waiting.addListener(onWaiting);
    final shown = showBundleSheet<bool>(
      context,
      BundleProgressSheet(
        progress: progress,
        closable: newApp,
        onCancel: newApp
            ? null
            : () {
                if (!cancel.isCompleted) cancel.complete();
              },
      ),
      dismissible: newApp,
    ).whenComplete(() => up = false);
    final out = await m.resolveDiverged(
      aid,
      choice,
      cancelled: cancel.future,
      onStep: (s) => progress.value = BundleWorking(
        headline,
        updateStepLabel(s),
        s != UpdateStep.publish && s != UpdateStep.liveMigrate,
      ),
      confirm: (merged) async =>
          context.mounted &&
          await showBundleSheet<bool>(
                context,
                MergedOfferSheet(
                  version: n ?? 0,
                  note: merged.note,
                  added: addedCapabilityWords(merged.added),
                  publishLabel: 'Publish to $host',
                ),
              ) ==
              true,
    );
    if (newApp) publisher.waiting.removeListener(onWaiting);
    final BundleProgress end;
    switch (out) {
      case DivergeDone(:final localVersion, :final remoteVersion, :final url):
        end = switch (choice) {
          DivergeChoice.combine => BundlePassed(
            'Published v$remoteVersion to $host',
            note: 'Your v$localVersion, with v$n',
          ),
          DivergeChoice.takeTheirs => BundlePassed('Took v$n from $host'),
          DivergeChoice.newApp => BundlePassed(
            '$title is a new app on $host',
            note: url,
          ),
        };
      case DivergeNotTaken(outcome: UpdateCancelled()):
        if (up) nav.pop();
        await shown;
        return;
      case DivergeNotTaken(
        outcome: UpdateCheckFailed(:final reason, :final details),
      ):
        end = updateCheckFailed(n ?? 0, reason, details: details);
      case DivergeNotTaken(outcome: UpdateRefused(:final message, :final details)):
        end = BundleFailed(message, details: details);
      case DivergeNotTaken():
        end = const BundleFailed('That could not finish.');
      case DivergeFailed(:final message, :final details):
        end = BundleFailed(
          message,
          details: context.mounted ? _developer(context, details) : const [],
        );
    }
    progress.value = end;
    if (!up) {
      // Closed while it went on: only how it ended badly is said.
      if (end is BundleFailed && context.mounted) {
        await showBundleSheet<bool>(
          context,
          BundleProgressSheet(progress: ValueNotifier(end)),
        );
      }
      return;
    }
    if (await shown == true) open(aid);
  }

  /// Tell @owner for linked app [aid] whose update [version] failed for
  /// [reason] (§5.7): null unless this device is signed in at the copy's
  /// remote, the copy takes comments, and the account may comment on it
  /// (Can comment or above; an author always may).
  Future<TellOwner?> tellOwner(String aid, int version, String reason) async {
    final link = await _resolved(aid);
    final rid = link?.remoteId;
    if (link == null || rid == null) return null;
    final raid = link.remoteArtifactId;
    final host = Uri.parse(link.remoteOrigin).host;
    final RemoteHead head;
    try {
      head = await remote.head(rid, raid);
      if (!head.comments) return null;
      if (!link.isAuthor) {
        String? access;
        for (final a in await remote.apps(rid)) {
          if (a.artifactId == raid) access = a.access;
        }
        if (!const {'commenter', 'editor', 'owner'}.contains(access)) {
          return null;
        }
      }
    } on ClaxApiError catch (e) {
      debugPrint('clax remote: who may comment on $raid: ${e.message}');
      return null;
    }
    return TellOwner(
      owner: head.owner.handle,
      draft: tellOwnerDraft(version, reason),
      post: (body) async {
        try {
          await remote.comment(rid, raid, body);
          return null;
        } on ClaxApiError catch (e) {
          if (e.code == 'comments_off') {
            return 'Comments are off on this app now.';
          }
          return remoteFailureLine(
            e,
            host: host,
            action: 'post comments',
            otherwise: 'The comment wasn’t posted: ${userMessage(e)}',
          );
        }
      },
    );
  }

  /// Share… for linked app [aid]: its sharing sheet, or, with no remote
  /// this device is signed in to, its page.
  Future<void> share(BuildContext context, String aid) async {
    final link = await _resolved(aid);
    if (link == null || !context.mounted) return;
    final title = (await store.entry(aid))?.title ?? '';
    if (!context.mounted) return;
    final rid = link.remoteId;
    if (rid == null) {
      await openUrl?.call(Uri.parse(link.url));
      return;
    }
    final s = shareCopy;
    if (s != null) {
      await s(
        context,
        rid: rid,
        raid: link.remoteArtifactId,
        title: title,
      );
      return;
    }
    await showShareSheet(
      context,
      remote,
      rid: rid,
      raid: link.remoteArtifactId,
      title: title,
      host: Uri.parse(link.url).host,
    );
  }

  /// Sets whether new versions of linked app [aid] publish without
  /// asking.
  Future<void> setAutoPublish(String aid, bool on) => _setAuto(aid, on);

  /// The head of [link]'s public copy: read with this device's token when
  /// it is signed in at the link's origin, else without.
  Future<RemoteHead> headOf(AppLink link) async {
    final r = await publisher.links.at(link.remoteOrigin);
    return r != null
        ? remote.head(r.id, link.remoteArtifactId)
        : remote.anonymousHead(Uri.parse(link.url));
  }

  /// [aid]'s link through the remote signed in to at its origin now
  /// ([LinkRemotes.resolve]); its stored link, its remote ID null, when
  /// that cannot be made.
  Future<AppLink?> _resolved(String aid) async {
    try {
      return await publisher.links.resolve(aid);
    } on ClaxApiError catch (e) {
      debugPrint('clax remote: $aid could not be linked again: ${e.message}');
      return (await store.entry(aid))?.link?.copyWith(remoteId: () => null);
    }
  }

  Future<void> _setAuto(String aid, bool on) => store.update(
    aid,
    (e) async => switch (e?.link) {
      final l? => e!.copyWith(link: () => l.copyWith(autoPublish: on)),
      null => null,
    },
  );

  /// Unpublish… for linked app [aid]: asks, then deletes the public copy
  /// and unlinks the app here. Copies others installed keep working.
  Future<void> unpublish(BuildContext context, String aid) async {
    final entry = await store.entry(aid);
    final link = await _resolved(aid);
    final rid = link?.remoteId;
    if (link == null || entry == null || !context.mounted) return;
    final name = Uri.parse(link.url).host;
    final yes = await ClaxDialog.show(
      context,
      title: 'Unpublish ${entry.title}?',
      body:
          'This deletes it on $name. Copies people installed keep working, '
          'but they stop getting updates. ${entry.title} stays here as it '
          'is.',
      actions: const [
        ClaxDialogAction('Cancel'),
        ClaxDialogAction('Unpublish', kind: ClaxButtonKind.destructive),
      ],
      cancelIndex: 0,
    );
    if (yes != 1 || !context.mounted) return;
    if (rid == null) {
      await showBundleSheet<bool>(
        context,
        BundleNoticeSheet(
          title: 'Not connected',
          message: 'Connect to $name in Settings › Sharing first.',
        ),
      );
      return;
    }
    // A publish of it still waiting to try again would make it public
    // again: it stops first.
    publisher.stop(aid);
    try {
      await remote.unpublish(rid, link.remoteArtifactId);
    } on ClaxApiError catch (e) {
      if (!context.mounted) return;
      await _failed(
        context,
        e.status == 403
            ? 'Only the owner of ${entry.title} can unpublish it.'
            : remoteUnreachable(e)
            ? 'Couldn’t reach $name. ${entry.title} is still on it.'
            : remoteFailureLine(
                e,
                host: name,
                action: 'unpublish apps',
                otherwise: 'Couldn’t unpublish ${entry.title}.',
              ),
        e,
      );
      return;
    }
    await store.update(
      aid,
      (e) async => e?.copyWith(link: () => null),
    );
    unlinked?.call(aid);
  }

  /// Get my apps: the signed-in account's apps, to pick and bring down,
  /// each new one after its install sheet agrees.
  Future<void> getMyApps(BuildContext context) async {
    final r = await _connected(context);
    if (r == null || !context.mounted) return;
    await Navigator.of(context).push<void>(
      MaterialPageRoute(
        builder: (screen) => LibraryScreen(
          remote: remote,
          rid: r.id,
          host: r.name,
          pull: (app) => puller.pullFromLibrary(
            r.id,
            app,
            confirm: (opened) => _confirmInstall(screen, opened),
          ),
        ),
      ),
    );
  }

  /// The install sheet for [opened], which says who it is from and what
  /// it may do: whether the person installs it.
  Future<bool> _confirmInstall(
    BuildContext context,
    svc.OpenedBundle opened,
  ) async {
    if (!context.mounted) return false;
    final ok = await showBundleSheet<bool>(
      context,
      InstallSheet(
        bundle: BundleSummary.of(opened),
        kind: switch (opened.decision.answer) {
          TrustAnswer.foreign => InstallKind.foreign,
          TrustAnswer.forked => InstallKind.forked,
          _ => InstallKind.install,
        },
      ),
    );
    return ok == true;
  }

  /// Gets the app at [url] (a Get in Clax link): the install sheet, which
  /// says who it is from, then the install; an update the app here takes
  /// goes to the update sheet.
  Future<void> getLink(BuildContext context, Uri url) async {
    final out = await puller.pull(
      url,
      confirm: (opened) => _confirmInstall(context, opened),
    );
    if (!context.mounted) return;
    switch (out) {
      case PullInstalled(:final aid) || PullHave(:final aid):
        open(aid);
      case PullUpdate(:final opened):
        await bundles?.offerUpdate(context, opened);
      case PullCancelled():
        break;
      case PullRefused(:final message):
        await showBundleSheet<bool>(
          context,
          BundleNoticeSheet(title: 'Couldn’t get this app', message: message),
        );
    }
  }

  Future<_Versions> _detail(String aid) async {
    final d = await exporter.api.getArtifact(aid);
    return (
      current: d.artifact.currentVersion,
      files: {
        for (final v in d.versions)
          v.n: (
            count: v.files.length,
            bytes: v.files.values.fold<int>(0, (n, f) => n + f.size),
          ),
      },
    );
  }

  Future<void> _failed(BuildContext context, String message, Object e) {
    debugPrint('clax remote: $message ${rawError(e)}');
    return showBundleSheet<bool>(
      context,
      BundleProgressSheet(
        progress: ValueNotifier(
          BundleFailed(message, details: _developer(context, [rawError(e)])),
        ),
      ),
    );
  }

  /// [raw], a failure's raw text, as its Details: only with Developer tools
  /// on (spec 2026-10-19 §7.3), else none.
  static List<String> _developer(BuildContext context, List<String> raw) =>
      DeveloperScope.of(context) ? raw : const [];
}

/// An app's current version, and each version's file count and bytes.
typedef _Versions = ({
  int current,
  Map<int, ({int count, int bytes})> files,
});
