import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// Tauri expects a fixed dev port and relative asset paths.
export default defineConfig({
  plugins: [svelte()],
  base: './',
  clearScreen: false,
  server: { port: 5173, strictPort: true },
});
