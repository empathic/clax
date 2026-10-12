/// The scripted agent of the macOS integration test (`full_loop_test.dart`):
/// the app it makes and the changes it answers with, in call order.
library;

import 'dart:async';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/edit_result.dart';
import 'package:clax_app/agent/request.dart';
import 'package:clax_app/agent/scripted_backend.dart';

/// The page's heading in the first version, and after each change.
const firstTitle = 'Runs';
const changedTitle = 'My runs';
const recoveredTitle = 'All runs';

/// The vertical scroll position the page moves to on its first visit,
/// which every later version must come back to.
const scrolledTo = 600;

/// The made app: it declares `db`, seeds `runs/r1 {km: 5}` on its first
/// visit (and scrolls to [scrolledTo] then), and shows the run.
///
/// The test cannot read the page's document (it is cross-origin to the
/// shell), so the page reports what it shows in the document
/// `probe/view`: its heading, `scrollY` and the run's text, written on
/// load and after each scroll settles.
const indexHtml =
    '''<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Run log</title>
<style>
:root { --bg: #ffffff; --fg: #1d1d1f; }
@media (prefers-color-scheme: dark) { :root { --bg: #1d1d1f; --fg: #f5f5f7; } }
body { margin: 0; background: var(--bg); color: var(--fg); font: 16px/1.4 system-ui, sans-serif; }
h1 { position: sticky; top: 0; margin: 0; height: 96px; line-height: 96px; padding: 0 16px; background: var(--bg); }
#km { padding: 16px; }
.track { height: 3000px; }
</style>
</head>
<body>
<h1 id="t">$firstTitle</h1>
<p id="km">…</p>
<div class="track"></div>
<script>
(async () => {
  const db = await window.claude.use("db");
  if (!db) return;
  const heading = document.getElementById("t");
  const km = document.getElementById("km");
  const probe = () => db.doc("probe/view").set({
    title: heading.textContent, y: Math.round(scrollY), shown: km.textContent,
  }).catch(() => {});
  let run = await db.doc("runs/r1").get();
  if (!run.exists) {
    await db.doc("runs/r1").set({ km: 5 });
    run = await db.doc("runs/r1").get();
    scrollTo(0, $scrolledTo);
  }
  km.textContent = run.data().km + " km";
  let settle;
  addEventListener("scroll", () => { clearTimeout(settle); settle = setTimeout(probe, 150); });
  probe();
})();
</script>
</body>
</html>
''';

/// A change that renames the heading from [from] to [to] and resolves its
/// comments.
EditResult rename(String from, String to) => EditResult(
  reply: 'Renamed the heading to "$to".',
  edits: [TextEdit(path: 'index.html', oldStr: '>$from<', newStr: '>$to<')],
  resolve: true,
);

/// The scripted answers, in call order: the new app, the first change, and
/// the change the test interrupts by killing the daemon.
List<Object> script() => [
  const EditResult(
    reply: 'Made a run log.',
    title: 'Run log',
    files: [FileWrite(path: 'index.html', content: indexHtml)],
    capabilities: {'db': <String, Object?>{}},
  ),
  rename(firstTitle, changedTitle),
  // After Undo the page is the first version's again.
  rename(firstTitle, recoveredTitle),
];

/// [ScriptedBackend] whose call number [holdAt] (from 1) is held after
/// the script answers it, until [release]: the change is then under way,
/// with nothing published yet.
class HeldBackend implements AgentBackend {
  HeldBackend(List<Object> script, {required this.holdAt})
    : inner = ScriptedBackend(script);

  final ScriptedBackend inner;
  final int holdAt;
  var calls = 0;

  /// Completes when call [holdAt] is held.
  final held = Completer<void>();
  final _release = Completer<void>();

  void release() {
    if (!_release.isCompleted) _release.complete();
  }

  @override
  String get name => inner.name;

  @override
  Future<EditResult> run(AgentRequest request) async {
    final n = ++calls;
    final result = await inner.run(request);
    if (n == holdAt) {
      held.complete();
      await _release.future;
    }
    return result;
  }
}
