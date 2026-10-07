fn main() {
    // 不注入 requireAdministrator：主进程按 asInvoker 运行，提权走 --task 子进程（设计方案 §1.4）
    tauri_build::build()
}
