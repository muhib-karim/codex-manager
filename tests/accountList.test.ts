import { describe, expect, it } from 'vitest';
import type { OpenAIAccount } from '../src/types/account';
import {
  clampPage,
  matchesSearch,
  pageCount,
  pageRange,
  pruneSelection,
  safeFileName,
  serializeAccountsForExport,
} from '../src/utils/accountList';

const account = (name: string, extra: Partial<OpenAIAccount> = {}): OpenAIAccount => ({
  id: name,
  name,
  apiKey: `sk-${name}-0123456789abcdef`,
  isActive: false,
  createdAt: 1,
  ...extra,
});

describe('pagination', () => {
  it('counts pages and never returns zero', () => {
    expect(pageCount(0, 12)).toBe(1);
    expect(pageCount(12, 12)).toBe(1);
    expect(pageCount(13, 12)).toBe(2);
    expect(pageCount(30, 12)).toBe(3);
    expect(pageCount(5, 0)).toBe(1);
  });

  // Regression: staying on page 3 after a search narrowed the list to 5
  // items rendered an empty grid.
  it('clamps the page into the valid range', () => {
    expect(clampPage(3, 5, 12)).toBe(1);
    expect(clampPage(3, 30, 12)).toBe(3);
    expect(clampPage(4, 30, 12)).toBe(3);
    expect(clampPage(0, 30, 12)).toBe(1);
    expect(clampPage(-2, 30, 12)).toBe(1);
    expect(clampPage(Number.NaN, 30, 12)).toBe(1);
    expect(clampPage(2.7, 30, 12)).toBe(2);
  });

  // Regression: the footer read "Showing 25 to 5 of 5".
  it('gives a consistent "showing X to Y" range', () => {
    expect(pageRange(1, 30, 12)).toEqual({ start: 1, end: 12 });
    expect(pageRange(3, 30, 12)).toEqual({ start: 25, end: 30 });
    expect(pageRange(3, 5, 12)).toEqual({ start: 1, end: 5 });
    expect(pageRange(1, 0, 12)).toEqual({ start: 0, end: 0 });
  });
});

describe('pruneSelection', () => {
  // Regression: deleting an account left its id selected, so the toolbar
  // still counted it and batch actions failed on it.
  it('drops ids that are no longer available', () => {
    const selected = new Set(['a', 'b', 'c']);
    const pruned = pruneSelection(selected, ['b', 'c', 'd']);
    expect([...pruned].sort()).toEqual(['b', 'c']);
    expect(selected.size).toBe(3); // input is not mutated
    expect(pruneSelection(selected, []).size).toBe(0);
  });
});

describe('matchesSearch', () => {
  it('matches name and organization, case-insensitively', () => {
    const acc = account('Production', { organization: 'org-Acme' });
    expect(matchesSearch(acc, '')).toBe(true);
    expect(matchesSearch(acc, '  ')).toBe(true);
    expect(matchesSearch(acc, 'prod')).toBe(true);
    expect(matchesSearch(acc, 'ACME')).toBe(true);
    expect(matchesSearch(acc, 'staging')).toBe(false);
  });

  // Regression: the search box matched raw API keys.
  it('never matches on the API key', () => {
    const acc = account('Production');
    expect(matchesSearch(acc, 'sk-')).toBe(false);
    expect(matchesSearch(acc, '0123456789')).toBe(false);
  });
});

describe('export', () => {
  it('writes one format with only the fields needed to re-import', () => {
    const data = JSON.parse(
      serializeAccountsForExport([
        account('alpha', { baseUrl: 'https://api.openai.com/v1', organization: 'org-1', disabled: true, lastUsed: 5 }),
      ]),
    );
    expect(data).toEqual([
      {
        name: 'alpha',
        apiKey: 'sk-alpha-0123456789abcdef',
        baseUrl: 'https://api.openai.com/v1',
        organization: 'org-1',
      },
    ]);
  });

  it('makes account names safe for file names', () => {
    expect(safeFileName('a/b:c')).toBe('a-b-c');
    expect(safeFileName('  My Account  ')).toBe('My-Account');
    expect(safeFileName('../../etc/passwd')).toBe('etc-passwd');
    expect(safeFileName('///')).toBe('account');
    expect(safeFileName('x'.repeat(200)).length).toBe(60);
  });
});
