//! 构建脚本：把要安装的程序放进 OUT_DIR，main.rs 用 include_bytes! 嵌入。
//! - CHRONICLE_PAYLOAD：已经用 xz 压缩好的程序（打包脚本用 LZMA 最高压缩生成，比 gzip 小三分之一）；
//! - CHRONICLE_PAYLOAD_SIZE：解压后的字节数（显示进度、写入「应用和功能」的大小）。
//!
//! 没给时嵌入一个空的（只用于开发调试界面，不能真正安装）。

fn main() {
    println!("cargo:rerun-if-env-changed=CHRONICLE_PAYLOAD");
    println!("cargo:rerun-if-changed=ui");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    println!("cargo:rerun-if-env-changed=CHRONICLE_PAYLOAD_SIZE");
    let (data, size) = match std::env::var("CHRONICLE_PAYLOAD").ok().filter(|p| !p.is_empty()) {
        Some(p) => {
            println!("cargo:rerun-if-changed={p}");
            let xz = std::fs::read(&p).unwrap_or_else(|e| panic!("读不了要打包的程序 {p}：{e}"));
            let size: u64 = std::env::var("CHRONICLE_PAYLOAD_SIZE").ok().and_then(|v| v.parse().ok()).expect("需要 CHRONICLE_PAYLOAD_SIZE（解压后的大小）");
            (xz, size)
        }
        None => (Vec::new(), 0),
    };
    std::fs::write(out.join("payload.xz"), &data).unwrap();
    std::fs::write(out.join("payload_size.txt"), size.to_string()).unwrap();
    tauri_build::build();
}
