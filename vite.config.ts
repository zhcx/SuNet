import { defineConfig } from "vite";

// Tauri 前端构建配置：产物固定输出到 dist/，由 tauri.conf.json 的 frontendDist 引用
//
// 两个入口：
//   index.html  主窗口
//   quick.html  托盘快捷面板（独立入口而不是查询串，见 README「已知偏差」：
//               Tauri 会把非 URL 形式的窗口 url 当路径处理，查询串在部分场景会丢）
export default defineConfig({
  clearScreen: false,
  server: {
    // 不用 Tauri 默认的 5173：本机上常有其他前端项目占用该端口，
    // 一旦被占，strictPort 会让 vite 直接退出，而 webview 会连到别人的页面上，
    // 表现为"界面空白但进程正常"这种极难排查的现象
    port: 5180,
    strictPort: true,
  },
  build: {
    target: "chrome110",
    outDir: "dist",
    emptyOutDir: true,
    sourcemap: false,
    minify: "esbuild",
    // 入口用相对路径（相对项目根），避免为 __dirname 引入 @types/node
    rollupOptions: {
      input: {
        main: "index.html",
        quick: "quick.html",
      },
    },
  },
});
