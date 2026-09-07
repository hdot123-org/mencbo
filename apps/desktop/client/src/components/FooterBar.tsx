import { FolderOpen, LogOut } from "lucide-react";
import { inTauri } from "../lib/env";

export function FooterBar() {
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
      <button
        data-testid="quit-app"
        onClick={handleQuit}
        className="flex items-center gap-2 px-3 py-1.5 text-xs font-medium text-neutral-300 hover:text-neutral-100 hover:bg-neutral-800 rounded transition-colors"
      >
        <LogOut size={14} />
        退出 MenCbo
      </button>
    </div>
  );
}
