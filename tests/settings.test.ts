import { describe, expect, it, vi } from 'vitest';
import { defaultConfig } from '../src/types/config';
import {
  applyTheme,
  mergeConfig,
  nextTheme,
  normalizeSchedulingMode,
  parseBoundedInt,
  parseStoredConfig,
  resolveTheme,
} from '../src/utils/settings';

describe('parseBoundedInt', () => {
  // Regression: `parseInt(x) || 8080` accepted -1, 0 and 70000 as ports and
  // negative refresh intervals.
  it('clamps into the range and falls back for non-numbers', () => {
    expect(parseBoundedInt('3000', 1, 65535, 8080)).toBe(3000);
    expect(parseBoundedInt('70000', 1, 65535, 8080)).toBe(65535);
    expect(parseBoundedInt('0', 1, 65535, 8080)).toBe(1);
    expect(parseBoundedInt('-3', 1, 60, 5)).toBe(1);
    expect(parseBoundedInt('', 1, 65535, 8080)).toBe(8080);
    expect(parseBoundedInt('abc', 1, 60, 5)).toBe(5);
    expect(parseBoundedInt('12.5', 1, 60, 5)).toBe(5);
    expect(parseBoundedInt(' 15 ', 1, 60, 5)).toBe(15);
  });
});

describe('mergeConfig', () => {
  it('returns the defaults for missing or unusable input', () => {
    expect(mergeConfig(undefined)).toEqual(defaultConfig);
    expect(mergeConfig(null)).toEqual(defaultConfig);
    expect(mergeConfig('nope')).toEqual(defaultConfig);
    expect(mergeConfig([])).toEqual(defaultConfig);
  });

  // Regression: a shallow merge dropped nested defaults, so a stored
  // `proxy` without `scheduling` crashed the settings page.
  it('merges nested sections key by key', () => {
    const merged = mergeConfig({ theme: 'dark', proxy: { port: 9090 }, notifications: { enabled: false } });
    expect(merged.theme).toBe('dark');
    expect(merged.proxy.port).toBe(9090);
    expect(merged.proxy.host).toBe(defaultConfig.proxy.host);
    expect(merged.proxy.scheduling).toEqual(defaultConfig.proxy.scheduling);
    expect(merged.proxy.rateLimiting).toEqual(defaultConfig.proxy.rateLimiting);
    expect(merged.notifications.enabled).toBe(false);
    expect(merged.notifications.quotaAlerts).toBe(defaultConfig.notifications.quotaAlerts);
  });

  it('rejects values of the wrong type or out of range', () => {
    const merged = mergeConfig({
      theme: 'neon',
      refreshInterval: -3,
      language: 42,
      proxy: { port: 70000, host: null, scheduling: { maxRetries: 'many' } },
    });
    expect(merged.theme).toBe('system');
    expect(merged.refreshInterval).toBe(1);
    expect(merged.language).toBe('en');
    expect(merged.proxy.port).toBe(65535);
    expect(merged.proxy.host).toBe('127.0.0.1');
    expect(merged.proxy.scheduling.maxRetries).toBe(3);
  });

  it('keeps the optional export path and ignores unknown keys', () => {
    const merged = mergeConfig({ defaultExportPath: '/tmp/exports', somethingElse: true });
    expect(merged.defaultExportPath).toBe('/tmp/exports');
    expect('somethingElse' in merged).toBe(false);
  });

  it('does not modify the defaults', () => {
    const before = JSON.stringify(defaultConfig);
    const merged = mergeConfig({ proxy: { scheduling: { mode: 'sticky' } } });
    merged.proxy.scheduling.maxRetries = 99;
    expect(JSON.stringify(defaultConfig)).toBe(before);
  });
});

describe('scheduling modes', () => {
  // Regression: the UI offered 'random' and 'weighted', which the backend
  // does not implement, and used different names for the others.
  it('maps stored values onto the four backend strategies', () => {
    expect(normalizeSchedulingMode('least_utilized')).toBe('least_utilized');
    expect(normalizeSchedulingMode('sticky')).toBe('sticky');
    expect(normalizeSchedulingMode('round-robin')).toBe('round_robin');
    expect(normalizeSchedulingMode('least-used')).toBe('least_utilized');
    expect(normalizeSchedulingMode('random')).toBe('least_utilized');
    expect(normalizeSchedulingMode('weighted')).toBe('least_utilized');
    expect(normalizeSchedulingMode(undefined)).toBe('least_utilized');
    expect(mergeConfig({ proxy: { scheduling: { mode: 'round-robin' } } }).proxy.scheduling.mode).toBe('round_robin');
    expect(mergeConfig({ proxy: { scheduling: { mode: 'weighted' } } }).proxy.scheduling.mode).toBe('least_utilized');
  });
});

describe('parseStoredConfig', () => {
  it('falls back to the defaults for missing or corrupt JSON', () => {
    expect(parseStoredConfig(null)).toEqual(defaultConfig);
    expect(parseStoredConfig('')).toEqual(defaultConfig);
    expect(parseStoredConfig('{ not json')).toEqual(defaultConfig);
    expect(parseStoredConfig('{"theme":"light"}').theme).toBe('light');
  });
});

describe('theme', () => {
  it('resolves the system theme from the OS preference', () => {
    expect(resolveTheme('system', true)).toBe('dark');
    expect(resolveTheme('system', false)).toBe('light');
    expect(resolveTheme('light', true)).toBe('light');
    expect(resolveTheme('dark', false)).toBe('dark');
  });

  // Regression: with theme "system" on a dark OS the toggle set "dark",
  // which changed nothing; it took two clicks to switch.
  it('toggles to the opposite of what is currently shown', () => {
    expect(nextTheme('system', true)).toBe('light');
    expect(nextTheme('system', false)).toBe('dark');
    expect(nextTheme('dark', false)).toBe('light');
    expect(nextTheme('light', true)).toBe('dark');
  });

  // Regression: only the `dark` class was updated, so `data-theme` (set by
  // the boot script) could disagree with it.
  it('applies the class and data-theme together', () => {
    const toggle = vi.fn();
    const setAttribute = vi.fn();
    const root = { classList: { toggle }, setAttribute };

    expect(applyTheme(root, 'dark', false)).toBe('dark');
    expect(toggle).toHaveBeenLastCalledWith('dark', true);
    expect(setAttribute).toHaveBeenLastCalledWith('data-theme', 'dark');

    expect(applyTheme(root, 'system', false)).toBe('light');
    expect(toggle).toHaveBeenLastCalledWith('dark', false);
    expect(setAttribute).toHaveBeenLastCalledWith('data-theme', 'light');
  });
});
