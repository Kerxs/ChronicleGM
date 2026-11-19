#!/usr/bin/env bash
# 在 Debian 12 上编译服务端（2GB 内存的机器请先开 swap，并用单线程编译）。
# 用法：在源码根目录执行 ./deploy/build-linux.sh
set -euo pipefail
command -v cargo >/dev/null || { echo "先安装 Rust：curl https://sh.rustup.rs -sSf | sh -s -- -y --profile minimal"; exit 1; }
sudo apt-get install -y build-essential pkg-config libssl-dev >/dev/null
if [ "$(free -m | awk '/Mem:/{print $2}')" -lt 3000 ] && ! swapon --show | grep -q .; then
  echo "内存不足 3GB 且没有 swap：sudo fallocate -l 2G /swapfile && sudo chmod 600 /swapfile && sudo mkswap /swapfile && sudo swapon /swapfile"
  exit 1
fi
CARGO_BUILD_JOBS=1 cargo build --release -p chronicle-server
ls -lh target/release/chronicle-server
