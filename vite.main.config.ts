import { defineConfig } from 'vite';

export default defineConfig({
  build: {
    rollupOptions: {
      // Native module — required from node_modules at runtime, not bundled.
      external: ['better-sqlite3'],
    },
  },
});
