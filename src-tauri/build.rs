fn main() {
    // 图标与配置变化时重新生成 Windows 资源（exe 内嵌的图标）。
    // 不写这两行时，换了图标后 Cargo 会沿用旧的 resource.lib，打出来的 exe 仍是旧图标（2026-11-15 实际发生过）。
    println!("cargo:rerun-if-changed=icons");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    tauri_build::build();
}
