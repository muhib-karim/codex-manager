import { describe, expect, it } from 'vitest';
import { parseAccountImport } from '../src/utils/importAccounts';

describe('parseAccountImport', () => {
  it('reads an array of account objects', () => {
    const result = parseAccountImport(
      JSON.stringify([
        { name: 'Work', apiKey: 'sk-work-12345678', organization: 'org-1' },
        { label: 'Backend shape', api_key: 'sk-backend-12345678', org_id: 'org-2' },
      ]),
    );
    expect(result.rejected).toEqual([]);
    expect(result.accounts).toEqual([
      { name: 'Work', apiKey: 'sk-work-12345678', baseUrl: undefined, organization: 'org-1' },
      { name: 'Backend shape', apiKey: 'sk-backend-12345678', baseUrl: undefined, organization: 'org-2' },
    ]);
  });

  it('reads an { accounts: [...] } export and a single object', () => {
    const wrapped = parseAccountImport('{"accounts":[{"name":"A","apiKey":"sk-aaaaaaaa"}]}');
    expect(wrapped.accounts.map(a => a.name)).toEqual(['A']);
    const single = parseAccountImport('{"name":"B","apiKey":"sk-bbbbbbbb"}');
    expect(single.accounts.map(a => a.apiKey)).toEqual(['sk-bbbbbbbb']);
  });

  it('reads plain text with one key per line and arrays of key strings', () => {
    const lines = parseAccountImport('sk-line-one-1234\n\n  sk-line-two-1234  \n');
    expect(lines.accounts).toEqual([
      { name: 'Imported Account 1', apiKey: 'sk-line-one-1234' },
      { name: 'Imported Account 2', apiKey: 'sk-line-two-1234' },
    ]);
    const strings = parseAccountImport('["sk-first-12345678","sk-second-1234567"]');
    expect(strings.accounts.map(a => a.apiKey)).toEqual(['sk-first-12345678', 'sk-second-1234567']);
  });

  // Regression: JSON with a typo fell through to line mode and every line
  // ("[", "{", '"name": "x",') was imported as an account.
  it('rejects malformed JSON as a whole', () => {
    const result = parseAccountImport('[\n  {\n    "name": "x"\n    "apiKey": "sk-abc"\n  }\n]');
    expect(result.accounts).toEqual([]);
    expect(result.rejected).toEqual([{ position: 1, reason: 'invalid JSON' }]);
  });

  // Regression: entries without a key were imported with apiKey ''.
  it('rejects entries without a usable key', () => {
    const result = parseAccountImport(
      JSON.stringify([{ name: 'No key' }, { name: 'Blank', apiKey: '   ' }, { name: 'Short', apiKey: 'sk-1' }, 42, null]),
    );
    expect(result.accounts).toEqual([]);
    expect(result.rejected.map(r => r.reason)).toEqual([
      'missing API key',
      'missing API key',
      'API key is too short',
      'not an account',
      'not an account',
    ]);
    expect(parseAccountImport('this is not a key').rejected[0].reason).toBe('API key contains whitespace');
  });

  // Regression: importing the same file twice doubled every account.
  it('skips keys that already exist or repeat within the import', () => {
    const result = parseAccountImport('sk-existing-1234\nsk-new-12345678\nsk-new-12345678', ['sk-existing-1234']);
    expect(result.accounts.map(a => a.apiKey)).toEqual(['sk-new-12345678']);
    expect(result.rejected).toEqual([
      { position: 1, reason: 'duplicate API key' },
      { position: 3, reason: 'duplicate API key' },
    ]);
  });

  it('returns nothing for empty input', () => {
    expect(parseAccountImport('   \n ')).toEqual({ accounts: [], rejected: [] });
  });
});
