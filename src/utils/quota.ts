import type { OpenAIAccount, QuotaData } from '../types/account';

/** Accounts with less than this share of their quota left count as "low". */
export const LOW_QUOTA_THRESHOLD = 0.2;

/**
 * Share of a quota that is left, in [0, 1].
 * Returns null when the quota is unknown (missing numbers or a total of 0),
 * so "no data" is never mistaken for "empty" or for an inflated ratio.
 */
export function remainingRatio(remaining: number | undefined, total: number | undefined): number | null {
    if (typeof remaining !== 'number' || typeof total !== 'number') return null;
    if (!Number.isFinite(remaining) || !Number.isFinite(total) || total <= 0) return null;
    return Math.min(1, Math.max(0, remaining / total));
}

/** Remaining share of the request quota, or null when unknown. */
export function requestQuotaRatio(quota: QuotaData | undefined): number | null {
    return remainingRatio(quota?.remainingRequests, quota?.totalRequests);
}

/** Remaining share of the token quota, or null when unknown. */
export function tokenQuotaRatio(quota: QuotaData | undefined): number | null {
    return remainingRatio(quota?.remainingTokens, quota?.totalTokens);
}

/** Whole percentage for a ratio, or null when the ratio is unknown. */
export function ratioToPercent(ratio: number | null): number | null {
    return ratio === null ? null : Math.round(ratio * 100);
}

/** Text for a percentage that may be unknown. */
export function formatPercent(percent: number | null): string {
    return percent === null ? 'N/A' : `${percent}%`;
}

/** An account is low on quota only when its quota is known and below the threshold. */
export function isLowQuota(account: OpenAIAccount): boolean {
    const ratio = requestQuotaRatio(account.quota);
    return ratio !== null && ratio < LOW_QUOTA_THRESHOLD;
}

/**
 * The account to recommend: an enabled account with the most quota left.
 * Exhausted accounts are never recommended. When no account has known quota,
 * the enabled account with the highest priority is suggested instead.
 */
export function pickBestAccount(accounts: OpenAIAccount[]): OpenAIAccount | undefined {
    const enabled = accounts.filter(account => !account.disabled);
    const byPriority = (a: OpenAIAccount, b: OpenAIAccount) =>
        (b.priority ?? 0) - (a.priority ?? 0) || a.createdAt - b.createdAt;

    const withQuota = enabled
        .map(account => ({ account, ratio: requestQuotaRatio(account.quota) }))
        .filter((entry): entry is { account: OpenAIAccount; ratio: number } => entry.ratio !== null && entry.ratio > 0)
        .sort((a, b) => b.ratio - a.ratio || byPriority(a.account, b.account));
    if (withQuota.length > 0) return withQuota[0].account;

    const unknown = enabled.filter(account => requestQuotaRatio(account.quota) === null).sort(byPriority);
    return unknown[0];
}

export interface AccountStats {
    total: number;
    active: number;
    disabled: number;
    lowQuota: number;
    totalRequests: number;
    totalTokens: number;
    /** Average remaining quota (percent) over enabled accounts with known quota, or null. */
    avgQuota: number | null;
}

/** Summary numbers for the dashboard and the accounts page. */
export function computeAccountStats(accounts: OpenAIAccount[]): AccountStats {
    const enabled = accounts.filter(account => !account.disabled);
    const knownRatios = enabled
        .map(account => requestQuotaRatio(account.quota))
        .filter((ratio): ratio is number => ratio !== null);

    return {
        total: accounts.length,
        active: accounts.filter(account => account.isActive).length,
        disabled: accounts.filter(account => account.disabled).length,
        lowQuota: enabled.filter(isLowQuota).length,
        totalRequests: accounts.reduce((sum, account) => sum + (account.usage?.dailyRequests ?? 0), 0),
        totalTokens: accounts.reduce((sum, account) => sum + (account.usage?.dailyTokens ?? 0), 0),
        avgQuota:
            knownRatios.length > 0
                ? Math.round((knownRatios.reduce((sum, ratio) => sum + ratio, 0) / knownRatios.length) * 100)
                : null,
    };
}
