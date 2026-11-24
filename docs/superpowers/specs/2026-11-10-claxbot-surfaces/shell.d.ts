// The surface file format (../2026-11-10-claxbot-surfaces-design.md) on
// ClaxShell's operations table (../../../../claxos/docs/claxbot-services.md,
// "What ClaxShell would register"). The tests of clax-surface, clax-cli and
// clax-bot read it (docs/BUILD.bazel, `claxbot_surface_examples`): keep it a
// file `clax bot surface check` accepts. Where that table gives no shape (a
// part, a window, the bridge's list), the shape here is a guess.

/**
 * You are working on a ClaxShell desktop. Each app is an artifact; the
 * bar, the dock and the launcher are parts of the shell, each an artifact
 * too. A part of the shell must call `claxos.call('shown')` once it has
 * drawn. Read `clax.shell.bridge()` before you write against the bridge: use
 * only the calls and topics it lists. After you publish a part, look at
 * it with `clax.shell.screenshot` before you say it is done.
 */
declare const shell: {
  /** Which artifact fills each part of the shell, and which is being worked on. @read */
  parts(): Promise<Part[]>;
  /** The windows open on the desktop. @read */
  windows(): Promise<Window[]>;
  /** The workspaces, and which is shown. @read */
  workspaces(): Promise<Workspace[]>;
  /** The bridge's calls and topics, as this desktop serves them. @read */
  bridge(): Promise<Bridge>;
  /** A part or a window as it draws now. @read @throws not_found */
  screenshot(of: { part: string } | { window: string }): Promise<Image>;
  settings: {
    /** The appearance settings. @read */
    get(): Promise<Settings>;
    /** Changes appearance settings; the fields left out keep their values. @write @throws invalid */
    set(patch: SettingsPatch): Promise<Settings>;
  };
  window: {
    /** Opens an app's window on the person's screen. @effect @throws not_found */
    open(artifact: string): Promise<Window>;
  };
  /** Shows the person a notification. @effect */
  notify(text: string): Promise<null>;
  /** Asks the person in the Comment box, and waits for their answer. @effect @waits @once @throws declined */
  ask(question: string, choices?: string[]): Promise<string>;
};

interface Part {
  /** `bar`, `dock`, `launcher`, … */
  name: string;
  artifact: string;
  title: string;
  /** A ClaxBot run is editing it now. */
  working: boolean;
}
interface Window { id: string; artifact: string; title: string; workspace: Int; focused: boolean }
interface Workspace { index: Int; shown: boolean; windows: Int }
interface Bridge {
  calls: { name: string; doc: string }[];
  topics: { name: string; doc: string }[];
}
interface Settings { theme: "light" | "dark"; accent: string; corners: Int }
interface SettingsPatch { theme?: "light" | "dark"; accent?: string; corners?: Int }

interface Kinds {
  // `threads.open` and `runs.wait` name operations of Clax's own surface that
  // come with fan-out, which `clax.d.ts` lacks, and a run of a host's kind has
  // no operation of Clax's (docs/contract.md, "Starting a run"): a run of
  // `triage` today has the `shell.*` operations alone, and `check` notes that
  // the two match nothing here.
  /**
   * The request is a comment the person drew across several apps. Decide
   * what each app it touches should be asked, and ask each with
   * `clax.threads.open`. Wait for those runs with `clax.runs.wait`, then
   * finish with what each did. Change no artifact yourself.
   * @allow shell.parts shell.windows shell.settings.get shell.screenshot shell.ask
   * @allow threads.open runs.wait
   */
  triage(request: Triage): TriageResult;
}
interface Triage {
  /** What the person wrote. */
  ask: Instruction;
  /** The apps and parts the drawn area covers. */
  touches: Touched[];
  /** The area as the person saw it. */
  clip?: Image;
}
interface Touched { artifact: string; title: string; part?: string }
interface TriageResult {
  asked: { artifact: string; run: string; outcome: string }[];
  summary: string;
}
