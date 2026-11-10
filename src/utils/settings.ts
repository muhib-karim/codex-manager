import {
    SCHEDULING_MODES,
    defaultConfig,
    type AppConfig,
    type SchedulingMode,
    type ThemeMode,
} from '../types/config';

export const CONFIG_STORAGE_KEY = 'codex-manager-config';

export const PORT_RANGE = { min: 1, max: 65535 } as const;
export const REFRESH_INTERVAL_RANGE = { min: 1, max: 60 } as const;

/**
 * Parse a number typed into an input. Returns `fallback` for anything that
 * is not a whole number and clamps the rest into [min, max].
 */
export function parseBoundedInt(raw: string, min: number, max: number, fallback: number): number {
    if (!/^\s*-?\d+\s*$/.test(raw)) return fallback;
    const value = Number.parseInt(raw, 10);
    if (!Number.isFinite(value)) return fallback;
    return Math.min(max, Math.max(min, value));
}

function boundedNumber(value: unknown, min: number, max: number, fallback: number): number {
    if (typeof value !== 'number' || !Number.isFinite(value)) return fallback;
    return Math.min(max, Math.max(min, Math.round(value)));
}

/** Names used by earlier versions of the settings page. */
const LEGACY_SCHEDULING_MODES: Record<string, SchedulingMode> = {
    'round-robin': 'round_robin',
    'least-used': 'least_utilized',
};

/** Map a stored scheduling mode onto one the backend supports. */
export function normalizeSchedulingMode(value: unknown): SchedulingMode {
    if (typeof value !== 'string') return defaultConfig.proxy.scheduling.mode;
    if ((SCHEDULING_MODES as readonly string[]).includes(value)) return value as SchedulingMode;
    return LEGACY_SCHEDULING_MODES[value] ?? defaultConfig.proxy.scheduling.mode;
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** Recursively overlay `stored` on `base`, keeping the shape of `base`. */
function overlay<T>(base: T, stored: unknown): T {
    if (!isRecord(base) || !isRecord(stored)) return base;
    const merged: Record<string, unknown> = { ...base };
    for (const [key, baseValue] of Object.entries(base)) {
        const storedValue = stored[key];
        if (storedValue === undefined || storedValue === null) continue;
        if (isRecord(baseValue)) {
            merged[key] = overlay(baseValue, storedValue);
        } else if (typeof storedValue === typeof baseValue) {
            merged[key] = storedValue;
        }
    }
    return merged as T;
}

/**
 * Combine stored settings with the defaults. Nested sections are merged
 * key by key, so settings saved by an older version keep working when new
 * keys are added; values of the wrong type or out of range fall back.
 */
export function mergeConfig(stored: unknown): AppConfig {
    const merged = overlay(defaultConfig, stored);
    const theme: ThemeMode = ['light', 'dark', 'system'].includes(merged.theme) ? merged.theme : defaultConfig.theme;
    // Optional setting that has no default value.
    const defaultExportPath =
        isRecord(stored) && typeof stored.defaultExportPath === 'string' ? stored.defaultExportPath : undefined;
    return {
        ...merged,
        ...(defaultExportPath ? { defaultExportPath } : {}),
        theme,
        refreshInterval: boundedNumber(
            merged.refreshInterval,
            REFRESH_INTERVAL_RANGE.min,
            REFRESH_INTERVAL_RANGE.max,
            defaultConfig.refreshInterval,
        ),
        proxy: {
            ...merged.proxy,
            port: boundedNumber(merged.proxy.port, PORT_RANGE.min, PORT_RANGE.max, defaultConfig.proxy.port),
            scheduling: {
                ...merged.proxy.scheduling,
                mode: normalizeSchedulingMode(merged.proxy.scheduling.mode),
            },
        },
    };
}

/** Read settings from their stored JSON text; bad JSON gives the defaults. */
export function parseStoredConfig(raw: string | null | undefined): AppConfig {
    if (!raw) return mergeConfig(undefined);
    try {
        return mergeConfig(JSON.parse(raw));
    } catch {
        return mergeConfig(undefined);
    }
}

/** The theme actually shown for a setting and the OS preference. */
export function resolveTheme(theme: ThemeMode, prefersDark: boolean): 'light' | 'dark' {
    if (theme === 'system') return prefersDark ? 'dark' : 'light';
    return theme;
}

/** The setting a theme toggle should switch to: the opposite of what is shown. */
export function nextTheme(theme: ThemeMode, prefersDark: boolean): 'light' | 'dark' {
    return resolveTheme(theme, prefersDark) === 'dark' ? 'light' : 'dark';
}

interface ThemeRoot {
    classList: { toggle(name: string, force?: boolean): unknown };
    setAttribute(name: string, value: string): void;
}

/** Apply a theme to the document root: both the `dark` class and `data-theme`. */
export function applyTheme(root: ThemeRoot, theme: ThemeMode, prefersDark: boolean): 'light' | 'dark' {
    const resolved = resolveTheme(theme, prefersDark);
    root.classList.toggle('dark', resolved === 'dark');
    root.setAttribute('data-theme', resolved);
    return resolved;
}
