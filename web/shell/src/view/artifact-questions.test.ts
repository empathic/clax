import { afterEach, describe, expect, it, vi } from "vitest";

const artifact = vi.fn(() => () => {});
vi.mock("../q", () => ({ artifact }));
const owner = vi.fn(async (_signal?: AbortSignal) => true);
vi.mock("../owner", () => ({ ownerBrowser: (s?: AbortSignal) => owner(s) }));
vi.mock("../stream", () => ({ pageStream: () => ({}) }));

const { artifactQuestions } = await import("./artifact-questions");

afterEach(() => { vi.useRealTimers(); artifact.mockClear(); owner.mockClear(); });

describe("artifactQuestions", () => {
  it("loads the question module for the owner once the browser is idle, and ends it with the view", async () => {
    vi.useFakeTimers();
    const ended = vi.fn();
    artifact.mockReturnValueOnce(ended);
    const stop = artifactQuestions("7q3k9mzx2b4t");
    expect(owner).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(2000);
    await vi.waitFor(() => expect(artifact).toHaveBeenCalledWith("7q3k9mzx2b4t", expect.anything(), expect.anything()));
    stop();
    expect(ended).toHaveBeenCalledOnce();
  });

  it("asks nothing and loads nothing once ended before the browser was idle", async () => {
    vi.useFakeTimers();
    artifactQuestions("7q3k9mzx2b4t")();
    await vi.advanceTimersByTimeAsync(5000);
    expect(owner).not.toHaveBeenCalled();
    expect(artifact).not.toHaveBeenCalled();
  });

  it("loads nothing for a browser that is not the owner's, and aborts a check still asking", async () => {
    vi.useFakeTimers();
    owner.mockImplementationOnce(async () => false);
    artifactQuestions("7q3k9mzx2b4t");
    await vi.advanceTimersByTimeAsync(2000);
    expect(owner).toHaveBeenCalledOnce();
    expect(artifact).not.toHaveBeenCalled();
    let signal: AbortSignal | undefined;
    owner.mockImplementationOnce(s => { signal = s; return new Promise(() => {}); });
    const stop = artifactQuestions("7q3k9mzx2b4t");
    await vi.advanceTimersByTimeAsync(2000);
    stop();
    expect(signal?.aborted).toBe(true);
  });
});
