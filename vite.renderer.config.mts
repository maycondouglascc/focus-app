import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'node:path';

// Reshaped CSS is processed by postcss.config.cjs (custom-media + cssnano).
// No Tailwind — the app is built on Reshaped components/tokens.
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, 'src'),
    },
  },
});
