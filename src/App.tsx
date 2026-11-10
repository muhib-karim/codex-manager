import { useEffect } from 'react';
import { BrowserRouter, Routes, Route } from 'react-router-dom';
import Layout from './components/layout/Layout';
import Dashboard from './pages/Dashboard';
import Accounts from './pages/Accounts';
import Settings from './pages/Settings';
import About from './pages/About';
import { useConfigStore } from './stores/useConfigStore';
import { applyTheme } from './utils/settings';

function App() {
    const theme = useConfigStore(state => state.config.theme);
    const loadConfig = useConfigStore(state => state.loadConfig);

    useEffect(() => {
        loadConfig();
    }, [loadConfig]);

    // Apply the theme on mount, when the setting changes, and when the OS
    // preference changes while the setting is "system".
    useEffect(() => {
        const media = window.matchMedia('(prefers-color-scheme: dark)');
        const apply = () => applyTheme(document.documentElement, theme, media.matches);
        apply();
        if (theme !== 'system') return;
        media.addEventListener('change', apply);
        return () => media.removeEventListener('change', apply);
    }, [theme]);

    return (
        <BrowserRouter>
            <Routes>
                <Route path="/" element={<Layout />}>
                    <Route index element={<Dashboard />} />
                    <Route path="accounts" element={<Accounts />} />
                    <Route path="settings" element={<Settings />} />
                    <Route path="about" element={<About />} />
                </Route>
            </Routes>
        </BrowserRouter>
    );
}

export default App;
