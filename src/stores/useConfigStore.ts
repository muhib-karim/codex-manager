import { create } from 'zustand';
import { AppConfig, defaultConfig } from '../types/config';
import { CONFIG_STORAGE_KEY, applyTheme, parseStoredConfig } from '../utils/settings';

const prefersDark = () =>
    typeof window !== 'undefined' && window.matchMedia('(prefers-color-scheme: dark)').matches;

const persist = (config: AppConfig) => localStorage.setItem(CONFIG_STORAGE_KEY, JSON.stringify(config));

interface ConfigState {
    config: AppConfig;
    loading: boolean;
    error: string | null;

    // Actions
    loadConfig: () => Promise<void>;
    saveConfig: (config: AppConfig) => Promise<void>;
    updateTheme: (theme: AppConfig['theme']) => void;
    updateLanguage: (language: string) => void;
    updateProxy: (proxy: Partial<AppConfig['proxy']>) => void;
    updateNotifications: (notifications: Partial<AppConfig['notifications']>) => void;
    updateAppearance: (appearance: Partial<AppConfig['appearance']>) => void;
}

export const useConfigStore = create<ConfigState>((set, get) => ({
    config: defaultConfig,
    loading: false,
    error: null,

    loadConfig: async () => {
        set({ loading: true, error: null });
        try {
            // Stored settings are merged over the defaults section by section;
            // corrupt or outdated data falls back to the defaults.
            set({ config: parseStoredConfig(localStorage.getItem(CONFIG_STORAGE_KEY)), loading: false });
        } catch (error) {
            set({ config: defaultConfig, error: String(error), loading: false });
        }
    },

    saveConfig: async (config) => {
        set({ loading: true, error: null });
        try {
            persist(config);
            set({ config, loading: false });
        } catch (error) {
            set({ error: String(error), loading: false });
            throw error;
        }
    },

    updateTheme: (theme) => {
        const { config } = get();
        const newConfig = { ...config, theme };
        set({ config: newConfig });
        persist(newConfig);

        // Apply theme immediately (class and data-theme together)
        applyTheme(document.documentElement, theme, prefersDark());
    },

    updateLanguage: (language) => {
        const { config } = get();
        const newConfig = { ...config, language };
        set({ config: newConfig });
        persist(newConfig);
    },

    updateProxy: (proxy) => {
        const { config } = get();
        const newConfig = { 
            ...config, 
            proxy: { ...config.proxy, ...proxy } 
        };
        set({ config: newConfig });
        persist(newConfig);
    },

    updateNotifications: (notifications) => {
        const { config } = get();
        const newConfig = { 
            ...config, 
            notifications: { ...config.notifications, ...notifications } 
        };
        set({ config: newConfig });
        persist(newConfig);
    },

    updateAppearance: (appearance) => {
        const { config } = get();
        const newConfig = { 
            ...config, 
            appearance: { ...config.appearance, ...appearance } 
        };
        set({ config: newConfig });
        persist(newConfig);
    },
}));
