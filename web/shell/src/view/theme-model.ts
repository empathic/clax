// The theme (spec §8): follow the system, plus a switch that flips light and
// dark. A flip that lands on the system's own scheme clears the choice, so
// the shell follows the system again. The choice is a per-browser
// convenience in localStorage; every access may throw (private windows).
export type Scheme = "light" | "dark";
export type Choice = Scheme | null;
export const THEME_KEY = "clax.theme";

export function readChoice(): Choice {
  try {
    const v = localStorage.getItem(THEME_KEY);
    return v === "light" || v === "dark" ? v : null;
  } catch { return null; }
}

export function systemScheme(): Scheme {
  return typeof matchMedia === "function" && matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export const shownScheme = (choice: Choice, system: Scheme): Scheme => choice ?? system;

/** The choice after one press of the switch. */
export function flip(choice: Choice, system: Scheme): Choice {
  const next: Scheme = shownScheme(choice, system) === "dark" ? "light" : "dark";
  return next === system ? null : next;
}

/** Applies `c` to the document and remembers it (null forgets). */
export function applyChoice(c: Choice, root: HTMLElement = document.documentElement): void {
  if (c) root.dataset.theme = c;
  else delete root.dataset.theme;
  try {
    if (c) localStorage.setItem(THEME_KEY, c);
    else localStorage.removeItem(THEME_KEY);
  } catch { /* storage unavailable: the choice lasts for this page */ }
}
