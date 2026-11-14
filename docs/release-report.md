# 自动更新 · GitHub 仓库 · 0.0.1 发布 · 完成报告

日期：2026-11-14。

## 结果

- 仓库：https://github.com/Kerxs/ChronicleGM （公开）
- 发布：https://github.com/Kerxs/ChronicleGM/releases/tag/v0.0.1
  - `ChronicleGM_0.0.1_x64-setup.exe`：7,976,080 字节，SHA-256 `d7c57bf85f976054f4640c2b125dc309e05c7712ce5bdd13d03cac52675fbc0b`
  - `ChronicleGM_0.0.1_x64-setup.exe.sig`：更新签名
  - `latest.json`：软件检查更新时读取的文件
- 版本号统一改为 0.0.1：workspace、各 crate、`tauri.conf.json`、`package.json`。

## 自动更新

- **组件**：使用 Tauri 官方的 `tauri-plugin-updater` 2.14.1，由 Rust 命令驱动，界面没有直接调用更新插件的权限。
  - `update_check`：读取 `https://github.com/Kerxs/ChronicleGM/releases/latest/download/latest.json`，比较版本。
  - `update_install`：下载安装包，**校验 minisign 签名**（公钥写在 `tauri.conf.json` 里），先停止托管的模型，再运行安装程序（NSIS，passive 模式），然后重启。
- **界面**：
  - 启动约 4 秒后自动检查一次（可以在「模型与性能 → 模型服务 → 关于」里关闭），失败不打扰；
  - 有新版本时标题栏出现绿色的「新版本 x.y.z」；
  - 点开是更新对话框：更新说明、下载进度、「立即更新 / 稍后」；
  - 「关于」里可以手动「检查更新」，并显示结果或失败原因。
- **用户数据**：世界、战役、设置都在 `%LOCALAPPDATA%\ChronicleGM`，不在安装目录里，更新不会动。
- **签名私钥**：放在仓库外的 `D:\Claude\chronicle-gm-keys\chronicle-gm-updater.key`（没有设密码），**没有上传**。
  - 以后每次发布都必须用这把私钥签名；私钥丢失后，已安装的软件就收不到更新了。请备份这个文件，并且不要公开。
- **测试开关**：`CHRONICLE_UPDATE_DRY_RUN=1` 时把已发布的版本当作新版本，只下载并校验签名，**拒绝安装**；`CHRONICLE_UPDATE_PUBKEY` 可以换一个公钥做反向测试，只在测试开关打开时生效。`CHRONICLE_UPDATE_ENDPOINT` 可以换更新地址（正式版只允许 https）。

## 发布前的隐私处理（按你的选择）

- `docs/perf/phase8-real.log` 里有一处测试数据目录路径含 Windows 用户名，已替换为 `<测试数据目录>`。
- 含用户名的旧截图（`docs/ui-redesign/`、`docs/img/`、`docs/launch-and-images/*.png`、`docs/image-input/*.png`，测试地图除外）写进了 `.gitignore`，只保留在本机。所以这些报告里的图片链接在 GitHub 上看不到。
- 提交前检查了所有要提交的文件：没有用户名、邮箱、API Key、令牌或私钥。
- 提交作者：`Kerxs <141604552+Kerxs@users.noreply.github.com>`，只设置在这个仓库里，没有改全局 git 配置。

## 验证

| 检查 | 结果 |
|---|---|
| Rust 测试 / clippy | 511 个通过，0 失败（5 个需要真实模型的忽略）；0 警告 |
| 前端测试 | 36 个通过 |
| 发布版 E2E 走查 | 22/22 步通过，控制台无错误 |
| 发布后下载 `latest.json` 与安装包 | 与上传的文件完全一致（SHA-256 相同） |
| 软件内检查更新（正式模式） | 已是最新（返回空），1.1 秒 |
| 测试模式：从 GitHub 真实下载并校验签名 | 下载 7,976,080 字节，签名校验通过，1.4 秒；安装请求被拒绝（测试模式不安装） |
| 反向测试：换一个公钥 | 签名校验失败："The signature was created with a different key than the one provided"，没有进入安装 |

**没有测试的**：真正"从旧版本自动安装新版本"的完整过程。这需要在你的电脑上安装旧版、再发布一个更高的版本，我没有在你的系统上安装软件。下一次发布 0.0.2 时，已安装 0.0.1 的电脑会收到更新，那就是第一次完整实测。

## 已知问题

1. 已添加 MIT 许可证（`LICENSE`，版权人 Kerxs）。
2. 安装包没有代码签名证书（Authenticode），Windows SmartScreen 第一次运行时可能提示"未知发布者"。更新签名（minisign）和代码签名是两回事。
3. 发布流程在本机完成（打包、签名、上传），没有配置 GitHub Actions：私钥不需要上传到 GitHub。
