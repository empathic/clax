declare const fake: {
  /** How many times this was read, this time included. @read */
  count(): Promise<Int>;
  /** The value kept under a name, null when there is none. @read */
  get(name: string): Promise<string | null>;
  /** Keeps a value under a name. @write */
  set(name: string, value: string): Promise<Written>;
  /** Always fails. @read @throws broken */
  fail(): Promise<null>;
  /** Always refuses. @write @throws forbidden */
  refuse(name: string): Promise<null>;
  /** Asks the fake's person, who may take long to answer. @effect @waits @once */
  ask(question: string): Promise<string>;
  /** An operation the tests' bindings leave out. @write */
  hidden(name: string): Promise<null>;
};

interface Written {
  name: string;
  /** How many writes the fake has applied, this one included. */
  writes: Int;
}

interface Kinds {
  /**
   * The request is a job for the fake.
   * @allow fake.count fake.get fake.set fake.fail fake.refuse fake.ask
   */
  job(request: { ask: Instruction }): { summary: string };
  /**
   * The request is a case for the fake to judge.
   * @allow fake.count fake.get
   */
  verdict(request: { ask: Instruction }): Verdict;
}

/** The fake's verdict on the case. */
interface Verdict {
  /** What was decided, in a few words. */
  title: string;
  /** How sure the fake is, from 0 to 10. */
  confidence?: Int;
  status: "upheld" | "dismissed" | "deferred";
}
