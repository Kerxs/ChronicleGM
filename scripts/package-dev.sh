#!/usr/bin/env bash
# 打包开发版：先编译程序（不打 NSIS 包），再把它嵌进自研安装器，输出 release/ChronicleGM-dev-setup.exe。
set -euo pipefail
cd "$(dirname "$0")/.."
npx tauri build --no-bundle
# 用 LZMA（xz，最高压缩）压缩程序，比 gzip 小约三分之一。
EXE=target/release/chronicle-gm-app.exe
python -c "import lzma,sys; d=open(sys.argv[1],'rb').read(); open(sys.argv[2],'wb').write(lzma.compress(d, preset=9|lzma.PRESET_EXTREME, check=lzma.CHECK_CRC64))" "$EXE" target/release/payload.xz
CHRONICLE_PAYLOAD="$(pwd)/target/release/payload.xz" CHRONICLE_PAYLOAD_SIZE="$(stat -c %s "$EXE")" cargo build --release -p chronicle-setup
mkdir -p release
cp target/release/chronicle-setup.exe release/ChronicleGM-dev-setup.exe
ls -l release/ChronicleGM-dev-setup.exe
sha256sum release/ChronicleGM-dev-setup.exe
