import { defineConfig } from 'vite';
import { resolve } from 'path';

export default defineConfig({
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
