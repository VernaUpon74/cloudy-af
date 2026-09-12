import { defineConfig } from 'vite';
import { readFileSync } from 'fs';
import { resolve } from 'path';

// Build-time feature flags from an OPTIONAL, untracked local-flags.json
// ({"patches": true}). Default is off: the Patches tab ships disabled on
// GitHub builds; the ~/cloudy-af dev checkout enables it locally via that
// file until the feature passes testing. See AGENTS.md.
let localFlags = {};
try {
  localFlags = JSON.parse(readFileSync(resolve(__dirname, 'local-flags.json'), 'utf8'));
} catch {
  // no local flag file — defaults apply
}

export default defineConfig({
  define: {
    __CLOUDY_PATCHES__: JSON.stringify(localFlags.patches === true),
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ['**/src-tauri/**'],
    },
  },
  build: {
    target: 'es2021',
    outDir: 'dist',
    emptyOutDir: true,
    rollupOptions: {
      input: {
        main: resolve(__dirname, 'index.html'),
        bat: resolve(__dirname, 'bat.html'),
        power: resolve(__dirname, 'power.html'),
        tfr: resolve(__dirname, 'tfr.html'),
        pireg: resolve(__dirname, 'pireg.html'),
        monitor: resolve(__dirname, 'monitor.html'),
        firmware: resolve(__dirname, 'firmware.html'),
      },
      output: {
        entryFileNames: 'assets/[name]-[hash].js',
        chunkFileNames: 'assets/[name]-[hash].js',
        assetFileNames: 'assets/[name]-[hash][extname]',
      },
    },
  },
  optimizeDeps: {
    include: ['jquery', 'highcharts', 'highcharts-draggable-points'],
  },
});
