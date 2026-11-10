import type { OpenAIAccount } from '../types/account';

/** Number of pages needed for `totalItems` (at least 1). */
export function pageCount(totalItems: number, pageSize: number): number {
    if (pageSize <= 0) return 1;
    return Math.max(1, Math.ceil(totalItems / pageSize));
}

/** Keep a page number inside the valid range for the current list. */
export function clampPage(page: number, totalItems: number, pageSize: number): number {
    const last = pageCount(totalItems, pageSize);
    if (!Number.isFinite(page) || page < 1) return 1;
    return Math.min(Math.floor(page), last);
}

/** 1-based "showing X to Y" range for a page; 0 to 0 for an empty list. */
export function pageRange(page: number, totalItems: number, pageSize: number): { start: number; end: number } {
    if (totalItems <= 0 || pageSize <= 0) return { start: 0, end: 0 };
    const current = clampPage(page, totalItems, pageSize);
    return {
        start: (current - 1) * pageSize + 1,
        end: Math.min(current * pageSize, totalItems),
    };
}

/** Drop selected ids that are no longer in the list (deleted or filtered out). */
export function pruneSelection(selected: ReadonlySet<string>, availableIds: Iterable<string>): Set<string> {
    const available = new Set(availableIds);
    return new Set([...selected].filter(id => available.has(id)));
}

/**
 * Text search over the fields that are safe to match: name and organization.
 * The API key is deliberately not searchable, so a key cannot be probed
 * piece by piece through the search box.
 */
export function matchesSearch(account: OpenAIAccount, query: string): boolean {
    const needle = query.trim().toLowerCase();
    if (!needle) return true;
    return (
        account.name.toLowerCase().includes(needle) ||
        (account.organization ?? '').toLowerCase().includes(needle)
    );
}

/** The one export format: only the fields needed to re-create the accounts. */
export function serializeAccountsForExport(accounts: OpenAIAccount[]): string {
    return JSON.stringify(
        accounts.map(account => ({
            name: account.name,
            apiKey: account.apiKey,
            baseUrl: account.baseUrl,
            organization: account.organization,
        })),
        null,
        2,
    );
}

/** Make an account name safe to use inside a file name. */
export function safeFileName(name: string): string {
    const cleaned = name
        .trim()
        .replace(/[^A-Za-z0-9._-]+/g, '-')
        .replace(/^[-.]+|[-.]+$/g, '');
    return cleaned.slice(0, 60) || 'account';
}
