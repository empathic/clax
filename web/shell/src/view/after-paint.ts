/** Runs `fn` once the page has painted: after the next animation frame, or
 * after 100 ms where none comes (a hidden tab). The result cancels it. */
export function afterPaint(fn: () => void): () => void {
  let raf = 0;
  let later: ReturnType<typeof setTimeout> | undefined;
  const done = () => {
    clearTimeout(timeout);
    if (raf) cancelAnimationFrame(raf);
    later = setTimeout(fn);
  };
  const timeout = setTimeout(done, 100);
  if (typeof requestAnimationFrame === "function") raf = requestAnimationFrame(done);
  return () => { clearTimeout(timeout); clearTimeout(later); if (raf) cancelAnimationFrame(raf); };
}
