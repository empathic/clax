/// The sheet of an app's first publish to a hosted Clax (spec
/// 2026-10-21-app-push-pull §5.3).
library;

import 'package:flutter/material.dart';

import '../bundle_sheets.dart';
import '../kit/kit.dart';
import '../theme.dart';
import '../tokens.g.dart';

/// The person's choice on a [PublishSheet]: whether this publish carries
/// their data (never remembered), whether new versions publish without
/// asking, and a fork's [name].
@immutable
class PublishChoice {
  const PublishChoice({
    required this.includeData,
    required this.autoPublish,
    this.name,
  });

  final bool includeData;
  final bool autoPublish;

  /// The name a fork goes up as; null for an app's own publish.
  final String? name;
}

/// Publish `<title>` to [host] as @[login]: who can open it (Restricted, not
/// a choice here), Include my data (off; disabled unless [dataAllowed]),
/// Publish new versions (Ask me or Automatically), Comments (off), and
/// what goes up and what stays. Pops a [PublishChoice] for Publish, else
/// null.
///
/// With [forkName] it is Publish fork (spec 2026-10-21-app-push-pull
/// §5.12): a Name field prefilled with it ("<title> (yours)"), the app
/// going up as the person's own; Publish waits for a name.
class PublishSheet extends StatefulWidget {
  const PublishSheet({
    super.key,
    required this.title,
    required this.host,
    required this.login,
    required this.version,
    required this.fileCount,
    required this.fileBytes,
    required this.dataAllowed,
    this.forkName,
    this.svg,
    this.word,
    this.seed = '',
  });

  final String title;

  /// A fork's prefilled name; null for an app's own publish.
  final String? forkName;

  /// The remote's name (`clax.dev`).
  final String host;

  /// Who this device publishes as there; null leaves the line out.
  final String? login;

  /// The local version going up, its file count and their bytes.
  final int version;
  final int fileCount;
  final int fileBytes;

  /// False past the seed caps: Include my data is disabled.
  final bool dataAllowed;

  /// The app's icon (cleaned SVG) and word, for its face.
  final String? svg;
  final String? word;
  final String seed;

  @override
  State<PublishSheet> createState() => _PublishSheetState();
}

class _PublishSheetState extends State<PublishSheet> {
  var _data = false;
  var _auto = false;
  late final _name = TextEditingController(text: widget.forkName);

  @override
  void dispose() {
    _name.dispose();
    super.dispose();
  }

  bool get _fork => widget.forkName != null;

  @override
  Widget build(BuildContext context) {
    final w = widget;
    final theme = Theme.of(context);
    final muted = claxPalette(context).muted;
    Widget heading(String text) => Padding(
      padding: const EdgeInsets.only(top: ClaxSpace.s12, bottom: 4),
      child: Text(text, style: theme.textTheme.titleSmall),
    );
    Widget line(String text, {Key? key}) => Padding(
      padding: const EdgeInsets.only(bottom: 4),
      child: Text(
        text,
        key: key,
        style: theme.textTheme.bodyMedium?.copyWith(color: muted),
      ),
    );
    return BundleSheetFrame(
      face: BundleFace(
        seed: w.seed,
        title: w.title,
        word: w.word,
        svg: w.svg,
      ),
      title: _fork ? 'Publish fork' : 'Publish ${w.title}',
      subtitle: w.login == null
          ? 'to ${w.host}'
          : 'to ${w.host} as @${w.login}',
      body: [
        if (_fork) ...[
          ClaxTextField(
            key: const ValueKey('fork-name'),
            controller: _name,
            label: 'Name',
            helper: 'Your version of ${w.title}, as a new app',
            textCapitalization: TextCapitalization.sentences,
            onChanged: (_) => setState(() {}),
          ),
          const SizedBox(height: ClaxSpace.s4),
        ],
        heading('Who can open it'),
        Row(
          children: [
            Expanded(child: line('Only you, until you share it')),
            // The lock as an icon: its glyph is not in every system font.
            ExcludeSemantics(
              child: Icon(
                Icons.lock_outline_rounded,
                size: 16,
                color: theme.textTheme.bodyMedium?.color,
              ),
            ),
            const SizedBox(width: ClaxSpace.s4),
            Text('Restricted', style: theme.textTheme.bodyMedium),
          ],
        ),
        const SizedBox(height: ClaxSpace.s8),
        ClaxSwitch(
          key: const ValueKey('include-data'),
          label: 'Include my data',
          value: _data,
          onChanged: w.dataAllowed ? (v) => setState(() => _data = v) : null,
        ),
        if (!w.dataAllowed) line('Too much data to include'),
        heading('Publish new versions'),
        ClaxRadio<bool>(
          key: const ValueKey('auto-ask'),
          value: false,
          group: _auto,
          label: 'Ask me',
          onChanged: (v) => setState(() => _auto = v),
        ),
        ClaxRadio<bool>(
          key: const ValueKey('auto-on'),
          value: true,
          group: _auto,
          label: 'Automatically',
          onChanged: (v) => setState(() => _auto = v),
        ),
        heading('Comments'),
        line('Off: nobody can comment, whatever their access'),
        const SizedBox(height: ClaxSpace.s12),
        Text(
          'Goes up: v${w.version} · ${w.fileCount} '
          '${w.fileCount == 1 ? 'file' : 'files'} · ${byteSize(w.fileBytes)}'
          '${_data ? ' · your data' : ''}',
          style: theme.textTheme.bodyMedium,
        ),
        line(
          _data
              ? 'Stays here: earlier versions, threads, API key'
              : 'Stays here: your data, earlier versions, threads, API key',
        ),
      ],
      actions: [
        ClaxButton(
          label: 'Cancel',
          onPressed: () => Navigator.of(context).pop(),
        ),
        ClaxButton(
          key: const ValueKey('publish'),
          kind: ClaxButtonKind.primary,
          isDefault: true,
          label: 'Publish',
          onPressed: _fork && _name.text.trim().isEmpty
              ? null
              : () => Navigator.of(context).pop(
                  PublishChoice(
                    includeData: _data,
                    autoPublish: _auto,
                    name: _fork ? _name.text.trim() : null,
                  ),
                ),
        ),
      ],
    );
  }
}
