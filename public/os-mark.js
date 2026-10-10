// 首帧平台标记（速网 SuNet）
//
// 为什么是"单独一个文件 + 经典脚本"而不是内联在 HTML 里：
//   · 生产 CSP 是 `script-src 'self'`（tauri.conf.json），**内联脚本会被直接拦掉**；
//   · 而 <script type="module"> 是 deferred 的，等它跑完第一帧早画完了 —— 主窗口在
//     main.rs 的 setup() 里就 show() 出来了，用户会先看到"没给交通灯留位"的顶栏，
//     之后界面才"跳"一下。顶栏留白、隐藏自绘窗口按钮这些必须在首次绘制前生效。
//
// 判定条件与 src/platform.ts 的 IS_MACOS 保持一致：那边优先读这里打上的属性，
// 只有在属性缺失时（脚本被删 / 页面被别的入口加载）才回退 UA。
//
// 本文件走 public/：Vite 原样拷进 dist、URL 不做改写，开发与打包行为一致。
if (/Macintosh|Mac OS X/.test(navigator.userAgent)) {
  document.documentElement.dataset.os = "macos";
}
