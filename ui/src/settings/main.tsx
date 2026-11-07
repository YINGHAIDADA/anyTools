import { invoke } from '@tauri-apps/api/core';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import './settings.css';

// Uncaught errors go to the app log (Settings › About › Open logs).
const report = (e: unknown) => void invoke('overlay_metric', { kind: 'settings_error', data: { message: String((e as Error)?.stack ?? e) } }).catch(() => {});
window.addEventListener('error', (ev) => report(ev.error ?? ev.message));
window.addEventListener('unhandledrejection', (ev) => report(ev.reason));

createRoot(document.getElementById('root')!).render(<App />);
requestAnimationFrame(() => void invoke('overlay_metric', { kind: 'settings_ready', data: {} }).catch(() => {}));
