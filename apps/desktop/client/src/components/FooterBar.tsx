import { FolderOpen, LogOut, RefreshCw } from "lucide-react";
import { useEffect, useState } from "react";
import { inTauri } from "../lib/env";

export function FooterBar() {
  const [version, setVersion] = useState<string>("");
  const [updState, setUpdState] = useState<"idle" | "checking" | "latest">("idle");

  useEffect(() => {
    if (!inTauri) return;
    import("@tauri-apps/api/app")
      .then(({ getVersion }) => getVersion())
      .then((v) => setVersion(`v${v}`))
      .catch(() => setVersion(""));
  }, []);

  // "已是最新" 反馈 4 秒后回到常态
  useEffect(() => {
    if (updState !== "latest") return;
    const t = setTimeout(() => setUpdState("idle"), 4000);
    return () => clearTimeout(t);
  }, [updState]);

  const handleCheckUpdate = async () => {
    if (!inTauri) {
      console.log("[mock] check update");
      return;
    }
    setUpdState("checking");
    const { invoke } = await import("@tauri-apps/api/core");
    try {
      // 有新版本时安装后 app 自动重启，此 invoke 随旧进程一起结束——预期行为
      const r = await invoke<string>("check_update");
      setUpdState(r === "latest" ? "latest" : "idle");
    } catch (e) {
      console.error("check_update failed:", e);
      setUpdState("idle");
    }
  };

  const handleOpenLogs = async () => {
    if (!inTauri) {
      console.log("[mock] open logs directory");
      return;
    }
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("open_logs_dir");
  };

  const handleQuit = async () => {
    if (!inTauri) {
      console.log("[mock] quit app");
      return;
    }
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("quit_app");
  };

  return (
    <div className="flex items-center justify-between px-4 py-3 border-t border-neutral-800 bg-neutral-900">
      <button
        data-testid="open-logs"
        onClick={handleOpenLogs}
        className="flex items-center gap-2 px-3 py-1.5 text-xs font-medium text-neutral-300 hover:text-neutral-100 hover:bg-neutral-800 rounded transition-colors"
      >
        <FolderOpen size={14} />
        打开日志目录
      </button>
      <span
        data-testid="app-version"
        className="text-[11px] font-mono text-neutral-500 select-none"
        title={version ? `当前版本 ${version}，有新版本时自动升级` : undefined}
      >
        {version}
      </span>
      <div className="flex items-center gap-1">
        <button
          data-testid="check-update"
          onClick={handleCheckUpdate}
          disabled={updState === "checking"}
          className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium text-neutral-300 hover:text-neutral-100 hover:bg-neutral-800 rounded transition-colors disabled:opacity-50"
        >
          <RefreshCw
            size={14}
            className={updState === "checking" ? "animate-spin" : undefined}
          />
          {updState === "checking" ? "检查中…" : updState === "latest" ? "已是最新" : "检查更新"}
        </button>
        <button
          data-testid="quit-app"
          onClick={handleQuit}
          className="flex items-center gap-2 px-3 py-1.5 text-xs font-medium text-neutral-300 hover:text-neutral-100 hover:bg-neutral-800 rounded transition-colors"
        >
          <LogOut size={14} />
          退出 MenCbo
        </button>
      </div>
    </div>
  );
}
