import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// Why: trusty-analyzer embeds the built dist/ directly in the Rust binary via
// include_dir!, so we want a self-contained, relative-path-friendly bundle.
// What: emit assets relative to the served root, do not split chunks
// excessively, target modern browsers (since this is a developer-facing tool).
// Test: `npm run build` produces ui/dist/index.html and ui/dist/assets/*.
export default defineConfig({
  plugins: [svelte()],
  base: './',
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    target: 'es2022',
    sourcemap: false,
  },
  server: {
    port: 5173,
    proxy: {
      // Forward API calls to the analyzer daemon during dev.
      '/health': 'http://127.0.0.1:7879',
      '/indexes': 'http://127.0.0.1:7879',
      '/facts': 'http://127.0.0.1:7879',
    },
  },
});
