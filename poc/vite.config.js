import { defineConfig } from 'vite';
import { resolve } from 'node:path';

export default defineConfig({
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: {
    target: 'safari16',
    rollupOptions: {
      input: {
        overlay: resolve(import.meta.dirname, 'index.html'),
        panel: resolve(import.meta.dirname, 'panel.html'),
      },
    },
  },
});
