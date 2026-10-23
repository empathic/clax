/// The inspector Overview's "On clax.dev" card (spec
/// 2026-10-21-app-push-pull §5.3, §5.4).
library;

import 'dart:async';

import 'package:flutter/material.dart';

import '../../bundle/lineage_store.dart';
import '../../clax/api.dart' show ClaxApiError;
import '../../remote/models.dart';
import '../../remote/sync_state.dart';
import '../kit/kit.dart';
import '../theme.dart';
import '../tokens.g.dart';
import 'remote_flows.dart';

/// The words a sync state's pill shows; null for none.
String? syncStateLabel(SyncState s) => switch (s) {
  SyncState.onlyHere => null,
  SyncState.live => 'Live',
  SyncState.ahead => 'Newer here',
  SyncState.updateWaiting => 'Update waiting',
  SyncState.keptMine => 'Kept mine',
  SyncState.diverged => 'Changed in both places',
  SyncState.forked => 'Changed here',
};

/// The icon before a sync state's pill words (as the launcher's badge
/// draws it: the glyphs are not in every system font); null for none.
IconData? syncStateIcon(SyncState s) => switch (s) {
  SyncState.live => Icons.cloud_rounded,
  SyncState.ahead => Icons.arrow_upward_rounded,
  SyncState.updateWaiting => Icons.arrow_downward_rounded,
  SyncState.diverged || SyncState.forked => Icons.call_split_rounded,
  SyncState.onlyHere || SyncState.keptMine => null,
};

/// App [aid]'s public copy, from its lineage [entry]: for an app with no
/// link, **Publish to clax.dev…** (**Publish fork…** for an app installed
/// from someone else's); for a linked one, `"On <host>"` with the
/// sync state's pill (once the head is read), the address, **Share…** and
/// **Open on `<host>`**, **Resolve…** while diverged (the diverged sheet),
/// **Check for updates** and **Publish as a new app** (**Publish fork…**
/// for a recipient) once kept mine, **Publish fork…** while a recipient's
/// copy is a fork, and
/// for an author New versions (Ask me | Automatically) and
/// **Unpublish…**. [liveContentSha256] is the live
/// version's content hash, and [modified] whether the live version is
/// past what this device last published or took (`syncState`'s
/// `modified`). [onChanged] runs after a publish, an unpublish
/// or a change of New versions, so the caller reads the entry again.
class RemoteCard extends StatefulWidget {
  const RemoteCard({
    super.key,
    required this.flows,
    required this.aid,
    required this.entry,
    required this.liveContentSha256,
    required this.modified,
    required this.onChanged,
    this.svg,
  });

  final RemoteFlows flows;
  final String aid;
  final LineageEntry? entry;
  final String liveContentSha256;
  final bool modified;
  final VoidCallback onChanged;
  final String? svg;

  @override
  State<RemoteCard> createState() => _RemoteCardState();
}

class _RemoteCardState extends State<RemoteCard> {
  RemoteHead? _head;
  var _gone = false;

  @override
  void initState() {
    super.initState();
    unawaited(_loadHead());
  }

  @override
  void didUpdateWidget(RemoteCard old) {
    super.didUpdateWidget(old);
    final was = old.entry?.link;
    final now = widget.entry?.link;
    if (was?.remoteArtifactId != now?.remoteArtifactId ||
        was?.baseRemoteVersion != now?.baseRemoteVersion) {
      unawaited(_loadHead());
    }
  }

  Future<void> _loadHead() async {
    final link = widget.entry?.link;
    if (link == null) return;
    try {
      final head = await widget.flows.headOf(link);
      if (mounted) {
        setState(() {
          _head = head;
          _gone = false;
        });
      }
    } on ClaxApiError catch (e) {
      // Offline: no pill, and nothing else changes.
      if (mounted && e.status == 404) setState(() => _gone = true);
    } on Object catch (e) {
      // An answer that could not be read: no pill either.
      debugPrint('clax remote: the head of ${link.url} was not read: $e');
    }
  }

  Future<void> _then(Future<void> work) async {
    await work;
    if (mounted) widget.onChanged();
  }

  @override
  Widget build(BuildContext context) {
    final f = widget.flows;
    final entry = widget.entry;
    final link = entry?.link;
    if (link == null) {
      return Align(
        alignment: Alignment.centerLeft,
        child: ClaxButton(
          key: const ValueKey('remote-publish'),
          icon: Icons.cloud_upload_rounded,
          label: f.publishLabelOf(entry),
          onPressed: () =>
              unawaited(_then(f.publish(context, widget.aid, svg: widget.svg))),
        ),
      );
    }
    final theme = Theme.of(context);
    final p = claxPalette(context);
    final host = Uri.parse(link.url).host;
    final state = syncState(
      link: link,
      liveContentSha256: widget.liveContentSha256,
      entry: entry,
      head: _head,
      modified: widget.modified,
    );
    final pill = _gone
        ? 'No longer on $host'
        : _head == null
        ? null
        : syncStateLabel(state);
    return DecoratedBox(
      key: const ValueKey('remote-card'),
      decoration: BoxDecoration(
        border: Border.all(color: p.border),
        borderRadius: BorderRadius.circular(ClaxRadius.md),
      ),
      child: Padding(
        padding: const EdgeInsets.all(ClaxSpace.s12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Expanded(
                  child: Text('On $host', style: theme.textTheme.titleSmall),
                ),
                if (pill != null) ...[
                  const SizedBox(width: ClaxSpace.s8),
                  if (!_gone && _head != null)
                    if (syncStateIcon(state) case final icon?) ...[
                      ExcludeSemantics(
                        child: Icon(
                          icon,
                          key: const ValueKey('remote-pill-icon'),
                          size: 14,
                          color: p.muted,
                        ),
                      ),
                      const SizedBox(width: ClaxSpace.s4),
                    ],
                  Flexible(
                    child: Text(
                      pill,
                      key: const ValueKey('remote-pill'),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: theme.textTheme.labelMedium?.copyWith(
                        color: p.muted,
                      ),
                    ),
                  ),
                ],
              ],
            ),
            const SizedBox(height: ClaxSpace.s4),
            SelectableText(
              link.url,
              style: theme.textTheme.bodyMedium?.copyWith(color: p.agent),
            ),
            const SizedBox(height: ClaxSpace.s8),
            Wrap(
              spacing: ClaxSpace.s8,
              runSpacing: ClaxSpace.s8,
              children: [
                if (link.isAuthor && !_gone)
                  ClaxButton(
                    key: const ValueKey('remote-share'),
                    label: 'Share…',
                    onPressed: () => unawaited(f.share(context, widget.aid)),
                  ),
                ClaxButton(
                  key: const ValueKey('remote-open'),
                  label: 'Open on $host',
                  onPressed: () =>
                      unawaited(f.openUrl?.call(Uri.parse(link.url))),
                ),
                if (!_gone &&
                    _head != null &&
                    state == SyncState.diverged &&
                    f.merger != null)
                  ClaxButton(
                    key: const ValueKey('remote-diverged'),
                    kind: ClaxButtonKind.primary,
                    label: 'Resolve…',
                    onPressed: () =>
                        unawaited(_then(f.diverged(context, widget.aid))),
                  ),
                if (!_gone && state == SyncState.forked)
                  ClaxButton(
                    key: const ValueKey('remote-fork'),
                    label: 'Publish fork…',
                    onPressed: () => unawaited(
                      _then(f.publish(context, widget.aid, svg: widget.svg)),
                    ),
                  ),
                if (!_gone && state == SyncState.keptMine) ...[
                  ClaxButton(
                    key: const ValueKey('remote-check-updates'),
                    label: 'Check for updates',
                    onPressed: () =>
                        unawaited(_then(f.update(context, widget.aid))),
                  ),
                  if (!link.isAuthor)
                    ClaxButton(
                      key: const ValueKey('remote-fork'),
                      label: 'Publish fork…',
                      onPressed: () => unawaited(
                        _then(f.publish(context, widget.aid, svg: widget.svg)),
                      ),
                    )
                  else if (f.merger != null && link.remoteId != null)
                    ClaxButton(
                      key: const ValueKey('remote-new-app'),
                      label: 'Publish as a new app',
                      onPressed: () => unawaited(
                        _then(f.publishAsNewApp(context, widget.aid)),
                      ),
                    ),
                ],
              ],
            ),
            if (link.isAuthor && !_gone) ...[
              const SizedBox(height: ClaxSpace.s8),
              Text('New versions', style: theme.textTheme.titleSmall),
              ClaxRadio<bool>(
                key: const ValueKey('remote-auto-ask'),
                value: false,
                group: link.autoPublish,
                label: 'Ask me',
                onChanged: (v) =>
                    unawaited(_then(f.setAutoPublish(widget.aid, v))),
              ),
              ClaxRadio<bool>(
                key: const ValueKey('remote-auto-on'),
                value: true,
                group: link.autoPublish,
                label: 'Automatically',
                onChanged: (v) =>
                    unawaited(_then(f.setAutoPublish(widget.aid, v))),
              ),
              const SizedBox(height: ClaxSpace.s8),
              ClaxButton(
                key: const ValueKey('remote-unpublish'),
                kind: ClaxButtonKind.destructive,
                label: 'Unpublish…',
                onPressed: () =>
                    unawaited(_then(f.unpublish(context, widget.aid))),
              ),
            ],
          ],
        ),
      ),
    );
  }
}
