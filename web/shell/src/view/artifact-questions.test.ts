import { afterEach, describe, expect, it, vi } from "vitest";

const artifact = vi.fn((..._a: unknown[]) => () => {});
const seed = vi.fn();
const firstRead = vi.fn(async () => {});
vi.mock("../q", () => ({ artifact, seed, firstRead }));
vi.mock("../stream", () => ({ pageStream: () => ({}) }));

const { QUESTIONS_WAIT_MS, artifactQuestions } = await import("./artifact-questions");

afterEach(() => { vi.useRealTimers(); artifact.mockClear(); seed.mockClear(); firstRead.mockClear(); });

describe("artifactQuestions", () => {
  it("seeds the feeds with the first read the page holds, mounts the surfaces, and says so once; ends them with the view", async () => {
    const ended = vi.fn();
    artifact.mockReturnValueOnce(ended);
    const decided = vi.fn();
    const onCount = vi.fn();
    const first = Promise.resolve({ summary: { unread: 2 } as never, open: [] });
    const stop = artifactQuestions("7q3k9mzx2b4t", first, onCount, decided);
    await vi.waitFor(() => expect(decided).toHaveBeenCalledOnce());
    expect(seed).toHaveBeenCalledWith({ unread: 2 }, []);
    expect(firstRead).not.toHaveBeenCalled();
    expect(artifact).toHaveBeenCalledWith("7q3k9mzx2b4t", expect.anything(), expect.anything(), document, onCount);
    stop();
    expect(ended).toHaveBeenCalledOnce();
    expect(decided).toHaveBeenCalledOnce();
  });

  it("reads the inbox itself where the page holds no first read", async () => {
    const decided = vi.fn();
    artifactQuestions("7q3k9mzx2b4t", null, () => {}, decided);
    await vi.waitFor(() => expect(decided).toHaveBeenCalledOnce());
    expect(firstRead).toHaveBeenCalledOnce();
    expect(artifact).toHaveBeenCalledOnce();
  });

  it("stops waiting QUESTIONS_WAIT_MS after it asked, on the injected clock, and mounts the surfaces when they come", async () => {
    vi.useFakeTimers();
    let answer: (f: { summary: null; open: null }) => void = () => {};
    const slow = new Promise<{ summary: null; open: null }>(r => { answer = r; });
    const decided = vi.fn();
    artifactQuestions("7q3k9mzx2b4t", slow, () => {}, decided);
    await vi.advanceTimersByTimeAsync(QUESTIONS_WAIT_MS - 1);
    expect(decided).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(decided).toHaveBeenCalledOnce();
    answer({ summary: null, open: null });
    await vi.waitFor(() => expect(artifact).toHaveBeenCalledOnce());
    expect(decided).toHaveBeenCalledOnce();
  });

  it("mounts nothing once ended before the module came", async () => {
    let answer: (f: { summary: null; open: null }) => void = () => {};
    const slow = new Promise<{ summary: null; open: null }>(r => { answer = r; });
    const decided = vi.fn();
    artifactQuestions("7q3k9mzx2b4t", slow, () => {}, decided)();
    expect(decided).toHaveBeenCalledOnce();
    answer({ summary: null, open: null });
    await vi.waitFor(() => expect(seed).toHaveBeenCalled());
    expect(artifact).not.toHaveBeenCalled();
  });
});
