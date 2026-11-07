import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { resolve } from 'node:path';

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 5174, strictPort: true },
  build: {
    target: 'safari16',
    rollupOptions: {
      input: {
        overlay: resolve(import.meta.dirname, 'overlay.html'),
        settings: resolve(import.meta.dirname, 'settings.html'),
      },
    },
  },
});
