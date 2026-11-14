import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import "./styles.css";
import "./shell.css";
import "./onboarding.css";
import "./cloud.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);

// 启动动画至少播放 1.4 秒（让动画完整），界面准备好后淡出并移除。
const MIN_MS = 1400;
const splash = document.getElementById("splash");
if (splash) {
  const wait = Math.max(0, MIN_MS - performance.now());
  setTimeout(() => {
    // 不用 requestAnimationFrame：窗口被遮挡时它会被节流，启动画面会停住。
    splash.classList.add("done");
    document.body.classList.add("app-ready");
    setTimeout(() => splash.remove(), 650);
  }, wait);
}
