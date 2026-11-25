// A paper test of the surface file format
// (../2026-11-10-claxbot-surfaces-design.md): Changeling's subject tools
// and review tasks (../../../../changelist/docs/remote-relay.md, "Subject
// tools" and "Review tasks"; web/src/lib/relayTools.ts;
// plugins/changeling/scripts/task.mjs) written as one file, with the
// structured comments and quest log its maintainers plan. The tests of
// clax-surface and clax-cli read it (docs/BUILD.bazel, `claxbot_surface_examples`).
// A run is bound to one subject (a pull request, a compare or a branch), so no
// operation names one.

/**
 * You are reviewing one subject on Changeling: a pull request, a compare
 * of two branches, or one branch's history. `clax.review.view()` says which.
 * Every result names the `head_sha` the subject had when it was read; a
 * branch moves, so compare it with the request's `head` before you rely
 * on line numbers. `from` and `to` are full commit SHAs on the subject's
 * track, `from` before `to`; `clax.review.commits()` lists them.
 */
declare const review: {
  /** The subject's description and state. @read @throws forbidden */
  view(): Promise<PullView | CompareView | RefView>;
  /** A pull request's conversation, newest first, then its review threads. @read @throws forbidden */
  comments(page?: Page): Promise<CommentPage>;
  /** The changed files, of the whole subject or of a range. @read @throws forbidden @throws off_track */
  files(range?: Range, page?: Page): Promise<FilePage>;
  /** The unified diff of one path, or of every file, as text content. @read @throws forbidden @throws off_track */
  diff(range?: Range, path?: string): Promise<Diff>;
  /** The commits, oldest first. @read @throws forbidden @throws off_track */
  commits(range?: Range, page?: Page): Promise<CommitPage>;
  /** Check runs and commit statuses of the head, or of commit `to`. @read @throws forbidden @throws off_track */
  checks(to?: string, page?: Page): Promise<CheckPage>;
  /** Says what you are doing, shown in the person's panel. @write */
  progress(text: string): Promise<null>;
  quest: {
    /** The quest log of this subject, if one was planned. @read */
    get(): Promise<Quest | null>;
    /** Marks a step read, with what you found there. @write @throws not_found */
    note(step: string, finding: string): Promise<Quest>;
  };
};

interface Page { cursor?: string; limit?: Int }
interface Range { from?: string; to?: string }
interface Named { subject: string; head_sha: string | null }

interface PullView extends Named {
  kind: "pull";
  url: string;
  title: string;
  body: string;
  state: "OPEN" | "CLOSED" | "MERGED";
  draft: boolean;
  author: string | null;
  base: { ref: string; sha: string };
  head: { ref: string; sha: string; repo: string | null };
  labels: { name: string; description: string | null }[];
  created: string;
  updated: string;
  mergeable: string;
  review_decision: string | null;
  additions: Int;
  deletions: Int;
  changed_files: Int;
}
interface CompareView extends Named {
  kind: "compare";
  url: string;
  base: { ref: string; sha: string; default_branch: boolean };
  head: { ref: string; sha: string; owner: string; repo: string | null };
  merge_base: string;
  status: string;
  ahead_by: Int;
  behind_by: Int;
  total_commits: Int;
  changed_files: Int;
  files_truncated: boolean;
  open_pull_requests: { number: Int; title: string; base: string }[];
}
interface RefView extends Named {
  kind: "ref";
  url: string;
  ref: { name: string; kind: "branch" | "tag" | "commit"; sha: string; default_branch: boolean };
  path: string | null;
  commits_read: Int;
  older_commits: boolean;
  open_pull_requests: { number: Int; title: string; base: string }[];
}

interface Listed extends Named { next_cursor: string | null; truncated: boolean }
interface CommentPage extends Listed {
  items: Comment[];
  total: Int | null;
}
interface Comment {
  kind: "comment" | "review" | "thread_comment";
  id: string;
  author: string | null;
  created: string;
  url: string;
  body: string;
  thread?: { id: string; path: string; line: Int | null; side: string; resolved: boolean; outdated: boolean };
}
interface FilePage extends Listed {
  from?: string;
  to?: string;
  files: { path: string; previous_path: string | null; status: string; additions: Int | null; deletions: Int | null }[];
  changed_files: Int | null;
}
interface Diff extends Named {
  from?: string;
  to?: string;
  /** The diff, as `text/x-diff`: read it a page at a time with `clax.content.text`. */
  diff: Content;
  truncated: boolean;
}
interface CommitPage extends Listed {
  from?: string;
  to?: string;
  commits: {
    sha: string;
    parents: string[];
    message: string | null;
    author: { name: string | null; login: string | null };
    authored: string | null;
    committed: string | null;
  }[];
  force_pushes?: { before: string; after: string; actor: string | null; at: string }[] | null;
}
interface CheckPage extends Listed {
  commit: string;
  state: string | null;
  checks: { type: "check" | "status"; name: string | null; status: string | null; conclusion: string | null; url: string | null }[];
}

interface Quest {
  steps: { id: string; title: string; why: string; path: string; start_line: Int; end_line: Int; commit: string; read: boolean }[];
}

interface Kinds {
  /**
   * The person selected lines, or sent you a review thread on them. Read
   * what you need, then answer about those lines.
   * @allow review.view review.files review.diff review.commits review.checks review.comments review.progress
   */
  range(request: RangeAsk): Answer;
  /**
   * The person asked about the whole subject, or a passage of its
   * description.
   * @allow review.view review.files review.diff review.commits review.checks review.comments review.progress
   */
  about(request: SubjectAsk): Answer;
  /**
   * Review the whole subject and write your findings as comments on its
   * lines. Publish nothing: each finding is a draft the person keeps or
   * drops.
   * @allow review.*
   */
  annotate(request: SubjectAsk): { findings: Finding[]; summary: string };
  /**
   * The subject is too large to read in one sitting. Plan the order a
   * person should read it in, as steps that each name lines and say why
   * they matter.
   * @allow review.view review.files review.diff review.commits review.progress
   */
  plan(request: SubjectAsk): Quest;
}

interface RangeAsk {
  /** The head the person was looking at. */
  head: string;
  path: string;
  side: "old" | "new";
  /** git's object ID of that side's file. */
  blob: string;
  start_line: Int;
  end_line: Int;
  /** The commit the text is read at. */
  commit: string;
  /** The review thread on those lines, if the person sent one. */
  thread?: { id: string; comments: { author: string | null; body: string }[] };
  /** What the person asked. */
  ask: Instruction;
  /** The earlier turns of a follow-up, oldest first. */
  conversation?: { from: "person" | "agent"; text: string }[];
}
interface SubjectAsk {
  head: string;
  /** A passage of the description the person asked about. */
  quote?: string;
  ask: Instruction;
  conversation?: { from: "person" | "agent"; text: string }[];
}
interface Answer {
  /** Markdown, kept as the person's private note on the lines or in the panel. */
  text: string;
  /** The head your answer is about. */
  head: string;
}
interface Finding {
  path: string;
  side: "old" | "new";
  start_line: Int;
  end_line: Int;
  commit: string;
  severity: "blocker" | "concern" | "suggestion" | "question" | "praise";
  body: string;
  /** A replacement for those lines, if you propose one. */
  suggestion?: string;
}
