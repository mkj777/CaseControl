import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

// Follows the Tauri 2 + Vite guide (v2.tauri.app/start/frontend/vite).
const host = process.env.TAURI_DEV_HOST
const debug = !!process.env.TAURI_ENV_DEBUG

export default defineConfig({
  plugins: [vue()],
  // Keep Rust compiler errors visible in the terminal.
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    host: host || false,
    watch: {
      // The Rust side is rebuilt by the Tauri CLI, not by Vite.
      ignored: ['**/src-tauri/**'],
    },
  },
  envPrefix: ['VITE_', 'TAURI_ENV_*'],
  build: {
    // WebView2 on Windows is Chromium based; chrome105 is the Tauri recommended floor.
    target: process.env.TAURI_ENV_PLATFORM === 'windows' ? 'chrome105' : 'safari13',
    minify: !debug,
    sourcemap: debug,
  },
})
