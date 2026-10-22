import { describe, expect, test } from 'vitest';
import { tileLabel } from '../home/grid.js';

// 7 px a character, in a cell 100 px wide: 14 characters to a line.
const opts = { measure: (s) => s.length * 7, width: 100 };
const words = (text) => new Set(text.split(/\s+/));

describe('tileLabel', () => {
  test('a title that fits is one line', () => {
    expect(tileLabel('Weekly budget', opts)).toEqual({ lines: ['Weekly budget'], ellipsis: false });
  });

  test('a longer title breaks between words, the rest on line 2, cut there', () => {
    expect(tileLabel('A tide table for the harbour', opts)).toEqual({ lines: ['A tide table', 'for the harbour'], ellipsis: true });
  });

  test('two lines that fit are not cut', () => {
    expect(tileLabel('Mood journal entries', opts)).toEqual({ lines: ['Mood journal', 'entries'], ellipsis: false });
  });

  test('a word wider than the cell is that word alone on one line, ellipsized', () => {
    const w = 'Donaudampfschifffahrtsgesellschaftskapitän';
    expect(w.length).toBe(42);
    expect(tileLabel(w, opts)).toEqual({ lines: [w], ellipsis: true });
    expect(tileLabel(`${w} log`, opts)).toEqual({ lines: [w], ellipsis: true });
  });

  test('nothing breaks mid-word', () => {
    for (const text of ['Kindergarten pickup rota', 'A tide table for the harbour', 'Habit streaks and more habits', 'Run tracker']) {
      const { lines } = tileLabel(text, opts);
      expect(lines.length).toBeLessThanOrEqual(2);
      for (const line of lines) for (const w of line.split(' ')) expect(words(text).has(w)).toBe(true);
    }
  });

  test('with lines: 1 the whole title is one line, cut when it does not fit', () => {
    expect(tileLabel('A packing list for trips', { ...opts, lines: 1 })).toEqual({ lines: ['A packing list for trips'], ellipsis: true });
    expect(tileLabel('A reading log', { ...opts, lines: 1 })).toEqual({ lines: ['A reading log'], ellipsis: false });
  });

  test('runs of spaces fold, and an empty title is no lines', () => {
    expect(tileLabel('  Weekly   budget ', opts)).toEqual({ lines: ['Weekly budget'], ellipsis: false });
    expect(tileLabel('   ', opts)).toEqual({ lines: [], ellipsis: false });
  });
});
