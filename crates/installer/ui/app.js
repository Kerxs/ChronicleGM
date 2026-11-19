const T = window.__TAURI__;
  const invoke = T.core.invoke;
  const win = () => T.window.getCurrentWindow();
  let info = null, busy = false, mode = "install";
  const $ = (id) => document.getElementById(id);
  const show = (id) => document.querySelectorAll(".step").forEach((s) => s.classList.toggle("on", s.id === id));
  function quit() { if (!busy) invoke("finish", { selfDelete: false }); }
  async function pick() {
    const d = await T.dialog.open({ directory: true, multiple: false, defaultPath: $("dir").value });
    if (typeof d === "string") $("dir").value = d.endsWith("Chronicle GM") ? d : d.replace(/[\\/]$/, "") + "\\Chronicle GM";
  }
  T.event.listen("progress", (e) => { $("bar").style.width = e.payload.pct + "%"; $("pstage").textContent = e.payload.stage; });
  async function doInstall() {
    $("ierr").textContent = "";
    busy = true; $("closeBtn").disabled = true; window.orbBusy && window.orbBusy(true);
    show("s-progress");
    try {
      await invoke("install", { dir: $("dir").value, desktop: $("optDesk").checked, startMenu: $("optStart").checked });
      $("dsub").textContent = "安装在 " + $("dir").value;
      show("s-done");
    } catch (e) {
      show("s-install"); $("ierr").textContent = String(e);
    } finally { busy = false; $("closeBtn").disabled = false; window.orbBusy && window.orbBusy(false); }
  }
  async function doUninstall() {
    $("uerr").textContent = "";
    if ($("optData").checked && !confirm("确定同时删除你的全部数据？世界、存档和下载的模型都会被永久删除，不能恢复。")) return;
    busy = true; $("closeBtn").disabled = true; window.orbBusy && window.orbBusy(true);
    $("ptitle").textContent = "正在卸载…";
    show("s-progress");
    try {
      await invoke("uninstall", { removeData: $("optData").checked });
      $("dtitle").textContent = "卸载完成";
      $("dsub").textContent = $("optData").checked ? "程序和数据都已删除。" : "程序已删除，你的数据还保留在 " + info.data_dir + "，重新安装后可以接着用。";
      $("launchRow").style.display = "none";
      show("s-done");
    } catch (e) {
      show("s-uninstall"); $("uerr").textContent = String(e);
    } finally { busy = false; $("closeBtn").disabled = false; window.orbBusy && window.orbBusy(false); }
  }
  async function done() {
    if (mode === "install" && $("optLaunch").checked) { try { await invoke("launch", { dir: $("dir").value }); } catch (e) { alert(e); } }
    invoke("finish", { selfDelete: mode === "uninstall" });
  }
  const acts = { min: () => win().minimize(), quit, pick, install: doInstall, uninstall: doUninstall, done };
  document.addEventListener("click", (e) => {
    const b = e.target.closest("[data-act]");
    if (b && !b.disabled) acts[b.dataset.act]();
  });
  (async () => {
    info = await invoke("info");
    mode = info.mode;
    if (mode === "uninstall") {
      $("wtitle").textContent = "卸载 Chronicle GM"; document.title = "卸载 Chronicle GM";
      $("dataDir").textContent = "数据位置：" + info.data_dir;
      show("s-uninstall");
    } else {
      $("dir").value = info.default_dir;
      $("ititle").textContent = info.installed ? "更新 Chronicle GM" : "安装 Chronicle GM";
      $("goInstall").textContent = info.installed ? "更新" : "安装";
      $("isub").textContent = "版本 " + info.version + (info.size_mb ? " · 约 " + info.size_mb + " MB" : "") + (info.installed ? " · 会覆盖已安装的旧版本" : "");
      show("s-install");
    }
  })();
