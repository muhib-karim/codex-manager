export interface ImportedAccount {
    name: string;
    apiKey: string;
    baseUrl?: string;
    organization?: string;
}

export interface RejectedImport {
    /** 1-based position of the entry in the input */
    position: number;
    reason: string;
}

export interface ImportResult {
    accounts: ImportedAccount[];
    rejected: RejectedImport[];
}

const MIN_KEY_LENGTH = 8;

function keyProblem(key: string): string | null {
    if (!key) return 'missing API key';
    if (/\s/.test(key)) return 'API key contains whitespace';
    if (key.length < MIN_KEY_LENGTH) return 'API key is too short';
    return null;
}

function asString(value: unknown): string | undefined {
    return typeof value === 'string' && value.trim() ? value.trim() : undefined;
}

/**
 * Parse pasted import data into accounts.
 *
 * Accepted input: a JSON array of accounts or of key strings, a JSON object
 * with an `accounts` array, a single JSON account object, or plain text with
 * one API key per line. Input that looks like JSON but does not parse is
 * rejected as a whole instead of being imported line by line.
 * Entries without a usable key and duplicates (of `existingKeys` or of an
 * earlier entry) are reported in `rejected`.
 */
export function parseAccountImport(text: string, existingKeys: Iterable<string> = []): ImportResult {
    const result: ImportResult = { accounts: [], rejected: [] };
    const trimmed = text.trim();
    if (!trimmed) return result;

    let entries: unknown[];
    if (trimmed.startsWith('[') || trimmed.startsWith('{')) {
        let parsed: unknown;
        try {
            parsed = JSON.parse(trimmed);
        } catch {
            result.rejected.push({ position: 1, reason: 'invalid JSON' });
            return result;
        }
        if (Array.isArray(parsed)) {
            entries = parsed;
        } else if (parsed && typeof parsed === 'object' && Array.isArray((parsed as { accounts?: unknown }).accounts)) {
            entries = (parsed as { accounts: unknown[] }).accounts;
        } else {
            entries = [parsed];
        }
    } else {
        entries = trimmed
            .split('\n')
            .map(line => line.trim())
            .filter(Boolean);
    }

    const seen = new Set(existingKeys);
    entries.forEach((entry, index) => {
        const position = index + 1;
        let candidate: ImportedAccount;

        if (typeof entry === 'string') {
            candidate = { name: `Imported Account ${position}`, apiKey: entry.trim() };
        } else if (entry && typeof entry === 'object' && !Array.isArray(entry)) {
            const record = entry as Record<string, unknown>;
            candidate = {
                name: asString(record.name) ?? asString(record.label) ?? `Imported Account ${position}`,
                apiKey: asString(record.apiKey) ?? asString(record.api_key) ?? '',
                baseUrl: asString(record.baseUrl),
                organization: asString(record.organization) ?? asString(record.org_id),
            };
        } else {
            result.rejected.push({ position, reason: 'not an account' });
            return;
        }

        const problem = keyProblem(candidate.apiKey);
        if (problem) {
            result.rejected.push({ position, reason: problem });
            return;
        }
        if (seen.has(candidate.apiKey)) {
            result.rejected.push({ position, reason: 'duplicate API key' });
            return;
        }
        seen.add(candidate.apiKey);
        result.accounts.push(candidate);
    });

    return result;
}
