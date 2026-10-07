// 快捷面板入口（quick.html）
//
// 与主窗口共用 api / ui / theme / icons，但外壳完全不同：
// 面板是 372×528 的无边框小窗，没有标签页，所有确认走内联条。

import { bootQuick } from "./quick";

void bootQuick();
