import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vitest/config';

// The screen for the Tauri app (M5): built into `dist`, which tauri.conf.json
// loads. `tauri dev` serves it from Vite on a fixed port instead.
export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: {
    outDir: 'dist',
    // WebKit on macOS 13+ and Android's WebView are both recent.
    target: 'es2022',
    sourcemap: false,
    // One bundle on purpose: it loads from the app itself, not a network.
    // TipTap, ProseMirror and Yjs make up most of its ~610 KB (200 KB gzipped).
    chunkSizeWarningLimit: 800,
  },
  test: { environment: 'node' },
});
