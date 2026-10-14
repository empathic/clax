/// The scripted tool loop of the macOS integration test
/// (`tool_loop_test.dart`): a counter app made and changed with the
/// agent's tools, rendered in the app's real inspector.
library;

import 'package:clax_app/agent/runner.dart';
import 'package:clax_app/agent/runner/scripted_runner.dart';
import 'package:clax_app/agent/tools/budget.dart';
import 'package:clax_app/agent/tools/toolbox.dart';

/// The counter's label in the first version, and after the change.
const firstLabel = 'Count';
const changedLabel = 'Taps';

/// The count the page seeds `counter/c` with on its first visit.
const seeded = 3;

/// The made app: it declares `db`, seeds `counter/c {n: [seeded]}` on its
/// first visit and counts up on "Add one".
///
/// The test cannot read the page's document (it is cross-origin to the
/// shell), so the page reports what it shows in the document
/// `probe/view`: its label and count, written on load and after each tap.
const counterHtml =
    '''<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Counter</title>
<style>
:root { --bg: #ffffff; --fg: #1d1d1f; --accent: #0a84ff; }
@media (prefers-color-scheme: dark) { :root { --bg: #1d1d1f; --fg: #f5f5f7; } }
body { margin: 0; background: var(--bg); color: var(--fg); font: 16px/1.4 system-ui, sans-serif; }
h1 { position: sticky; top: 0; margin: 0; height: 96px; line-height: 96px; padding: 0 16px; background: var(--bg); }
#n { padding: 0 16px; font-size: 48px; margin: 16px 0; }
button { margin: 0 16px; padding: 12px 20px; border: 0; border-radius: 10px; background: var(--accent); color: #fff; font: inherit; }
</style>
</head>
<body>
<h1 id="t">$firstLabel</h1>
<p id="n">…</p>
<button id="add">Add one</button>
<script>
(async () => {
  const label = document.getElementById("t");
  const shown = document.getElementById("n");
  const db = await window.claude.use("db");
  if (!db) return;
  const ref = db.doc("counter/c");
  let snap = await ref.get();
  if (!snap.exists) {
    await ref.set({ n: $seeded });
    snap = await ref.get();
  }
  let n = snap.data().n;
  const probe = () => db.doc("probe/view").set({
    label: label.textContent, shown: shown.textContent,
  }).catch(() => {});
  const show = () => { shown.textContent = String(n); probe(); };
  document.getElementById("add").addEventListener("click", async () => {
    n += 1;
    show();
    await ref.set({ n });
  });
  show();
})();
</script>
</body>
</html>
''';

/// The app's test, which `render` runs over a scratch copy of the
/// documents.
const counterTest = '''test("Add one counts up", async () => {
  await ui.waitFor(() => /^[0-9]+\$/.test(ui.text("#n")), 3000);
  const before = Number(ui.text("#n"));
  await ui.click("Add one");
  await ui.waitFor(() => Number(ui.text("#n")) === before + 1, 3000);
  expect(Number(ui.text("#n"))).toBe(before + 1);
});
''';

/// The new app: write the page and its test, name it and declare `db`,
/// render, finish.
const makeRun = [
  ScriptedCall('write_file', {'path': 'index.html', 'content': counterHtml}),
  ScriptedCall('write_file', {
    'path': 'tests/counter.test.js',
    'content': counterTest,
  }),
  ScriptedCall('set_meta', {
    'title': 'Counter',
    'capabilities_json': '{"db": {}}',
  }),
  ScriptedCall('render'),
  ScriptedCall('finish', {'reply': 'Made a counter.', 'resolve': true}),
];

/// The change: rename the label, render, finish.
const changeRun = [
  ScriptedCall('str_replace', {
    'path': 'index.html',
    'old_str': '>$firstLabel<',
    'new_str': '>$changedLabel<',
  }),
  ScriptedCall('render'),
  ScriptedCall('finish', {
    'reply': 'Renamed the label to "$changedLabel".',
    'resolve': true,
  }),
];

/// One tool call a [RecordingRunner] made, and what it answered.
class Call {
  const Call(this.run, this.tool, this.output);

  /// The run's number, from 1.
  final int run;
  final String tool;
  final ToolOutput output;

  @override
  String toString() => 'run $run $tool: $output';
}

/// Plays [runs] one per run on the run's real [Toolbox], as
/// [ScriptedRunner] does, and keeps every call's answer in [calls]; a run
/// past the last one gives up.
class RecordingRunner implements AgentRunner {
  RecordingRunner(List<List<ScriptedCall>> runs) : _runs = List.of(runs);

  final List<List<ScriptedCall>> _runs;
  final calls = <Call>[];
  var started = 0;

  @override
  String get name => 'scripted';

  @override
  bool get sendsImages => true;

  /// The answers of [tool] in run [run].
  List<ToolOutput> outputs(int run, String tool) => [
    for (final c in calls)
      if (c.run == run && c.tool == tool) c.output,
  ];

  @override
  Future<RunOutcome> run(RunInput input, Toolbox toolbox) async {
    final n = ++started;
    final script = _runs.isEmpty ? const <ScriptedCall>[] : _runs.removeAt(0);
    final state = toolbox.state;
    try {
      for (final c in script) {
        await Future<void>.value();
        if (state.outcome case final o?) return o;
        state.budget.addTurn(const Usage());
        calls.add(Call(n, c.tool, await toolbox.call(c.tool, c.input)));
      }
      await Future<void>.value();
      return state.outcome ??= const RunGaveUp(
        'the script ended without finishing',
        answered: false,
      );
    } finally {
      state.budget.endClock();
    }
  }
}
