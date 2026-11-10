import { describe, expect, it } from 'vitest';
import type { OpenAIAccount, QuotaData } from '../src/types/account';
import {
  computeAccountStats,
  formatPercent,
  isLowQuota,
  pickBestAccount,
  ratioToPercent,
  remainingRatio,
  requestQuotaRatio,
  tokenQuotaRatio,
} from '../src/utils/quota';

let counter = 0;
const account = (name: string, quota?: QuotaData, extra: Partial<OpenAIAccount> = {}): OpenAIAccount => ({
  id: name,
  name,
  apiKey: `sk-${name}-0123456789abcdef`,
  isActive: false,
  createdAt: ++counter,
  quota,
  ...extra,
});

describe('remainingRatio', () => {
  it('is null when the quota is unknown', () => {
    expect(remainingRatio(undefined, undefined)).toBeNull();
    expect(remainingRatio(500, undefined)).toBeNull();
    expect(remainingRatio(undefined, 1000)).toBeNull();
    expect(remainingRatio(5, 0)).toBeNull();
    expect(remainingRatio(Number.NaN, 10)).toBeNull();
    expect(requestQuotaRatio(undefined)).toBeNull();
  });

  // Regression: a missing total used to become 1, turning 500 remaining into 50000 %.
  it('never produces a ratio above 1 or below 0', () => {
    expect(remainingRatio(500, 1000)).toBe(0.5);
    expect(remainingRatio(1500, 1000)).toBe(1);
    expect(remainingRatio(-5, 1000)).toBe(0);
    expect(remainingRatio(0, 1000)).toBe(0);
  });

  it('reads request and token quotas separately', () => {
    const quota: QuotaData = { remainingRequests: 90, totalRequests: 100, remainingTokens: 1, totalTokens: 4 };
    expect(requestQuotaRatio(quota)).toBe(0.9);
    expect(tokenQuotaRatio(quota)).toBe(0.25);
  });

  it('formats unknown percentages as N/A', () => {
    expect(ratioToPercent(null)).toBeNull();
    expect(ratioToPercent(0.256)).toBe(26);
    expect(formatPercent(null)).toBe('N/A');
    expect(formatPercent(26)).toBe('26%');
    expect(formatPercent(0)).toBe('0%');
  });
});

describe('isLowQuota', () => {
  // Regression: accounts without quota data were counted as "low quota".
  it('is false when the quota is unknown', () => {
    expect(isLowQuota(account('nodata'))).toBe(false);
  });

  it('uses the 20 % threshold', () => {
    expect(isLowQuota(account('low', { remainingRequests: 19, totalRequests: 100 }))).toBe(true);
    expect(isLowQuota(account('edge', { remainingRequests: 20, totalRequests: 100 }))).toBe(false);
    expect(isLowQuota(account('empty', { remainingRequests: 0, totalRequests: 100 }))).toBe(true);
  });
});

describe('pickBestAccount', () => {
  // Regression: { remaining: 500, total: undefined } used to beat a 90 % account.
  it('prefers the highest known remaining share', () => {
    const noTotal = account('noTotal', { remainingRequests: 500 });
    const ninety = account('ninety', { remainingRequests: 900, totalRequests: 1000 });
    const half = account('half', { remainingRequests: 5, totalRequests: 10 });
    expect(pickBestAccount([noTotal, half, ninety])?.id).toBe('ninety');
  });

  it('skips disabled and exhausted accounts', () => {
    const disabled = account('disabled', { remainingRequests: 100, totalRequests: 100 }, { disabled: true });
    const exhausted = account('exhausted', { remainingRequests: 0, totalRequests: 100 });
    const ok = account('ok', { remainingRequests: 10, totalRequests: 100 });
    expect(pickBestAccount([disabled, exhausted, ok])?.id).toBe('ok');
    expect(pickBestAccount([disabled, exhausted])).toBeUndefined();
    expect(pickBestAccount([])).toBeUndefined();
  });

  it('breaks ties by priority and falls back to priority when no quota is known', () => {
    const a = account('a', { remainingRequests: 50, totalRequests: 100 }, { priority: 1 });
    const b = account('b', { remainingRequests: 50, totalRequests: 100 }, { priority: 5 });
    expect(pickBestAccount([a, b])?.id).toBe('b');

    const plain = account('plain');
    const preferred = account('preferred', undefined, { priority: 3 });
    expect(pickBestAccount([plain, preferred])?.id).toBe('preferred');
  });
});

describe('computeAccountStats', () => {
  it('returns zeros and a null average for an empty list', () => {
    expect(computeAccountStats([])).toEqual({
      total: 0, active: 0, disabled: 0, lowQuota: 0, totalRequests: 0, totalTokens: 0, avgQuota: null,
    });
  });

  // Regression: unknown quotas dragged the average to 0 % and counted as low.
  it('ignores unknown quotas and disabled accounts in the average and low count', () => {
    const stats = computeAccountStats([
      account('full', { remainingRequests: 100, totalRequests: 100 }, { isActive: true }),
      account('half', { remainingRequests: 50, totalRequests: 100 }),
      account('low', { remainingRequests: 10, totalRequests: 100 }),
      account('nodata'),
      account('off', { remainingRequests: 0, totalRequests: 100 }, { disabled: true }),
    ]);
    expect(stats.total).toBe(5);
    expect(stats.active).toBe(1);
    expect(stats.disabled).toBe(1);
    expect(stats.lowQuota).toBe(1);
    expect(stats.avgQuota).toBe(53); // (100 + 50 + 10) / 3
  });

  it('sums daily usage', () => {
    const usage = { dailyRequests: 3, dailyTokens: 40, monthlyRequests: 0, monthlyTokens: 0 };
    const stats = computeAccountStats([account('a', undefined, { usage }), account('b', undefined, { usage }), account('c')]);
    expect(stats.totalRequests).toBe(6);
    expect(stats.totalTokens).toBe(80);
    expect(stats.avgQuota).toBeNull();
  });
});
