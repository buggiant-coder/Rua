import { defineConfig, loadEnv } from "vite";
import react from "@vitejs/plugin-react";
import path from "path";

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), "");

  const apiBaseUrl = env.VITE_API_BASE_URL ?? "";
  const proxyTarget =
    env.VITE_PROXY_TARGET ?? `http://localhost:${env.RUA_DEV_PORT ?? "8765"}`;

  return {
    define: {
      VITE_API_BASE_URL: JSON.stringify(apiBaseUrl),
      TOKEN: JSON.stringify(env.TOKEN || ""),
      MOBILE: false,
    },
    plugins: [react()],
    css: {
      modules: {
        localsConvention: "camelCase",
        generateScopedName: "[name]__[local]__[hash:base64:5]",
      },
      preprocessorOptions: {
        less: {
          javascriptEnabled: true,
        },
      },
    },
    resolve: {
      alias: {
        "@": path.resolve(__dirname, "./src"),
      },
    },
    server: {
      host: "0.0.0.0",
      port: 5173,
      proxy: {
        "/api": {
          target: proxyTarget,
          // Preserve the browser-facing Host for the desktop same-origin guard.
          changeOrigin: false,
          ws: true,
        },
      },
    },
    optimizeDeps: {
      include: ["diff"],
    },
    build: {
      chunkSizeWarningLimit: 5000,
      rollupOptions: {
        output: {
          manualChunks: {
            "vendor-react": ["react", "react-dom", "react-router-dom"],
          },
        },
      },
    },
  };
});
