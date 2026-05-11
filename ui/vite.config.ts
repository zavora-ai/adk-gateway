import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  base: '/ui/',
  build: {
    outDir: 'dist',
  },
  server: {
    port: 5173,
    proxy: {
      '/ui/api': {
        target: 'http://localhost:18789',
        changeOrigin: true,
      },
      '/ws': {
        target: 'ws://localhost:18789',
        ws: true,
      },
    },
  },
})
