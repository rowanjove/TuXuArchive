import { defineConfig, type Plugin } from "vite";

function classicScript(): Plugin {
  return {
    name: "classic-script",
    apply: "build",
    transformIndexHtml(html) {
      const classic = html
        .replaceAll("<script type=\"module\" crossorigin ", "<script defer ")
        .replaceAll("<script type=\"module\" ", "<script defer ");
      return classic.includes("defer") ? classic : classic.replace("</body>", "<script defer src=\"./app.js\"></script></body>");
    },
  };
}

export default defineConfig({
  clearScreen: false,
  base: "./",
  server: {
    port: 1422,
    strictPort: true,
  },
  plugins: [classicScript()],
  build: {
    target: "es2022",
    cssCodeSplit: false,
    modulePreload: false,
    rollupOptions: {
      output: {
        format: "iife",
        inlineDynamicImports: true,
        entryFileNames: "app.js",
        assetFileNames: "app.[ext]",
      },
    },
  },
});
