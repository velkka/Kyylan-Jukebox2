import { resolve } from 'node:path'
import { defineConfig, externalizeDepsPlugin } from 'electron-vite'
import react from '@vitejs/plugin-react'

// Two renderers share one Vite build:
//   web/    -> the React UI served over HTTP to LAN guests and loaded in the host window
//   player/ -> a hidden window that actually plays audio through the host's output device
export default defineConfig({
  main: {
    plugins: [externalizeDepsPlugin()],
    resolve: {
      alias: { '@shared': resolve('src/shared') }
    },
    build: {
      rollupOptions: {
        input: { index: resolve('src/main/index.ts') }
      }
    }
  },
  preload: {
    plugins: [externalizeDepsPlugin()],
    build: {
      rollupOptions: {
        input: { index: resolve('src/preload/index.ts') }
      }
    }
  },
  renderer: {
    root: 'src/renderer',
    resolve: {
      alias: { '@shared': resolve('src/shared') }
    },
    server: {
      // In dev the UI is served by Vite; forward API + websocket to the
      // embedded Express server (default port 8080).
      proxy: {
        '/api': 'http://localhost:8080',
        '/ws': { target: 'http://localhost:8080', ws: true }
      }
    },
    plugins: [react()],
    build: {
      rollupOptions: {
        input: {
          web: resolve('src/renderer/web/index.html'),
          player: resolve('src/renderer/player/index.html')
        }
      }
    }
  }
})
