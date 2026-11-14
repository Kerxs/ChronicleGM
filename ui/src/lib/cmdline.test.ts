import { describe, expect, it } from "vitest";
import { parseCommandLine } from "./cmdline";

describe("parseCommandLine", () => {
  it("拆分你的 启动.bat 形式（^ 续行、引号、空变量）", () => {
    const bat = `@echo off
chcp 65001 >nul
cd /d D:\llama
set REASONING_ARGS=

"D:\llama\llama-server.exe" ^
-m "D:\llama\models\Ornith-1.5-35B-Q4_K_M.gguf" ^
-ngl auto ^
-c 32768 ^
--host 127.0.0.1 ^
--port 8080 %REASONING_ARGS%

pause`;
    const r = parseCommandLine(bat);
    expect(r.exe).toBe("D:\llama\llama-server.exe");
    expect(r.args).toEqual(["-m", "D:\llama\models\Ornith-1.5-35B-Q4_K_M.gguf", "-ngl", "auto", "-c", "32768", "--host", "127.0.0.1", "--port", "8080"]);
    expect(r.unresolved).toEqual([]);
    expect(r.port).toBe(8080);
  });
  it("替换 bat 里 set 的变量；没有定义的变量提示用户", () => {
    const bat = `set REASONING_ARGS=--reasoning-budget 2048
"D:\\llama\\llama-server.exe" ^
-c 32768 %REASONING_ARGS% %OTHER%`;
    const r = parseCommandLine(bat);
    expect(r.args).toEqual(["-c", "32768", "--reasoning-budget", "2048"]);
    expect(r.unresolved).toEqual(["%OTHER%"]);
  });
  it("单行与带空格的路径", () => {
    const r = parseCommandLine(`"C:\Program Files\tts\server.exe" --port 8137 --voice "女声 1"`);
    expect(r.exe).toBe("C:\Program Files\tts\server.exe");
    expect(r.args).toEqual(["--port", "8137", "--voice", "女声 1"]);
    expect(r.port).toBe(8137);
  });
});
