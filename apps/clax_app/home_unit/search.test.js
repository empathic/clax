import { describe, expect, test } from 'vitest';
import { fold, search } from '../home/search.js';

const e = (title, group = null) => ({ app: { id: title, title }, group });

describe('search', () => {
  const all = [e('Run tracker'), e('Groceries', 'Lists'), e('Café budget'), e('Brunch spots'), e('Habit streaks'), e('Strength')];

  test('folds case and accents', () => {
    expect(fold('Café ÉCLAIR')).toBe('cafe eclair');
  });

  test('word starts first, then anywhere, each in home order', () => {
    expect(search('run', all).map((x) => x.app.title)).toEqual(['Run tracker', 'Brunch spots']);
    expect(search('str', all).map((x) => x.app.title)).toEqual(['Habit streaks', 'Strength']);
    expect(search('CAFE', all).map((x) => x.app.title)).toEqual(['Café budget']);
    expect(search('ries', all).map((x) => x.app.title)).toEqual(['Groceries']);
  });

  test("keeps the group an app is in, and matches a whole phrase", () => {
    expect(search('groc', all)).toEqual([e('Groceries', 'Lists')]);
    expect(search('run tr', all).map((x) => x.app.title)).toEqual(['Run tracker']);
  });

  test('a blank query matches nothing', () => {
    expect(search('', all)).toEqual([]);
    expect(search('   ', all)).toEqual([]);
  });
});
