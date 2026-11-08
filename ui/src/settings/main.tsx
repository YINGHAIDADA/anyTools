import { invoke } from '@tauri-apps/api/core';
import { mockIPC } from '@tauri-apps/api/mocks';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import './settings.css';

// A plain browser has no Tauri backend. Stand in for it so the page can be previewed; inside the app
// `isTauri` is already set and this is skipped.
if (!('__TAURI_INTERNALS__' in window)) {
  mockIPC((cmd) => {
    switch (cmd) {
      case 'status':
        return { platform: 'mac', version: '0.1.0', hook_running: true, permissions: {}, liquid_glass: true, config_error: null, update: null };
      case 'get_config':
        return { version: 1, settings: { theme: 'dark', accent: 'system', terminal: null, material: 'liquid' }, modules: { wheel: { triggers: [], wheels: [], rules: [] } } };
      case 'module_call':
        return { geom: {}, triggers: [], wheels: [] };
      case 'list_scripts':
        return [];
      case 'autostart_get':
        return false;
      default:
        return null;
    }
  }, { shouldMockEvents: true });
}

// Uncaught errors go to the app log (Settings › About › Open logs).
const report = (e: unknown) => void invoke('overlay_metric', { kind: 'settings_error', data: { message: String((e as Error)?.stack ?? e) } }).catch(() => {});
window.addEventListener('error', (ev) => report(ev.error ?? ev.message));
window.addEventListener('unhandledrejection', (ev) => report(ev.reason));

createRoot(document.getElementById('root')!).render(<App />);
requestAnimationFrame(() => void invoke('overlay_metric', { kind: 'settings_ready', data: {} }).catch(() => {}));
