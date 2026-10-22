/// What the shell's messages say of a version the integration rig waits
/// for (`Rig.reloadNewer`), kept apart from the rig so a unit test can run
/// it.
library;

import 'package:clax_app/ui/host_protocol.dart';

/// How a version reached the app: [offered] by the shell's `newer` (the New
/// version toast swaps it in), or [shown] by a page that opened on it or a
/// later one (a new shell document needs no toast).
enum NewerSeen { offered, shown }

/// How [version] reached the app by [messages] (oldest first), from the
/// latest message that says: a `newer` of [version], or a `ready` on
/// [version] or later; null while neither has come.
NewerSeen? newerSeen(Iterable<HostMessage> messages, int version) {
  for (final m in messages.toList().reversed) {
    if (m is NewerMsg && m.version == version) return NewerSeen.offered;
    if (m is Ready && m.version >= version) return NewerSeen.shown;
  }
  return null;
}
