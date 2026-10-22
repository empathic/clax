import 'dart:convert';
import 'dart:math';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'kit/family.dart';
import 'theme.dart';
import 'tokens.g.dart';

/// Where the shared haiku list is in the app's assets.
const haikuAsset = 'assets/haiku.json';

/// The shared haiku list (assets/haiku.json, copied from the shell's
/// `web/shell/src/view/haiku.json`); empty when it is missing or not a list
/// of strings.
Future<List<String>> loadHaikus(AssetBundle bundle) async {
  try {
    final list = jsonDecode(await bundle.loadString(haikuAsset));
    if (list is List) return [for (final h in list) if (h is String) h];
  } on Object {
    // Missing or unreadable: no haikus.
  }
  return const [];
}

/// One haiku of [list] (which must not be empty): with a [seed], the same
/// one every time, picked by the seed's 32-bit FNV-1a hash as the shell's
/// `pickHaiku` (`web/shell/src/view/haiku.ts`) picks it; without one, at
/// random from [random].
String pickHaiku(List<String> list, {String? seed, Random? random}) {
  if (seed == null) return list[(random ?? Random()).nextInt(list.length)];
  var h = 0x811c9dc5;
  for (final unit in seed.codeUnits) {
    h ^= unit;
    h = (h * 0x01000193) & 0xffffffff;
  }
  return list[h % list.length];
}

/// A haiku, quiet: the `haiku` style in `muted`, its lines as written. A
/// screen reader reads it as "Haiku: …" when it is focused and never as a
/// live region. A long-press (Material) or an Option-click (Apple) calls
/// [onAnother]; a change of [text] fades across, unless motion is reduced.
class HaikuLine extends StatelessWidget {
  const HaikuLine({super.key, required this.text, this.onAnother});

  final String text;

  /// Asks for another haiku; null leaves this one in place.
  final VoidCallback? onAnother;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final style = ClaxType.of(
      ClaxRole.haiku,
      theme.platform,
    ).copyWith(color: claxPalette(context).muted);
    final still = MediaQuery.disableAnimationsOf(context);
    Widget line = AnimatedSwitcher(
      duration: still ? Duration.zero : const Duration(milliseconds: 300),
      layoutBuilder: (current, previous) => Stack(
        alignment: AlignmentDirectional.topStart,
        children: [...previous, ?current],
      ),
      child: Text(text, key: ValueKey(text), style: style),
    );
    final another = onAnother;
    if (another != null) {
      line = ClaxFamily.of(context) == ClaxFamily.apple
          ? GestureDetector(
              behavior: HitTestBehavior.opaque,
              onTap: () {
                if (_optionDown) another();
              },
              child: line,
            )
          : GestureDetector(
              behavior: HitTestBehavior.opaque,
              onLongPress: another,
              child: line,
            );
    }
    return Semantics(
      container: true,
      label: 'Haiku: $text',
      liveRegion: false,
      child: ExcludeSemantics(child: line),
    );
  }

  static bool get _optionDown {
    final keys = HardwareKeyboard.instance.logicalKeysPressed;
    return keys.contains(LogicalKeyboardKey.altLeft) ||
        keys.contains(LogicalKeyboardKey.altRight);
  }
}
