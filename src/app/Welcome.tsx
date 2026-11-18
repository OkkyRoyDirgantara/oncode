// Layar awal saat belum ada folder yang dibuka.
import { FolderOpen, GitBranch, Settings, TriangleAlert } from "lucide-react";
import { commands } from "@/ipc";
import { executeCommand } from "./commands";
import { showError } from "./overlay";
import { baseName, openFolder, pickAndOpenFolder, useApp } from "./store";

export function Welcome() {
  const info = useApp((s) => s.info);
  const recent = info?.recent ?? [];
  return (
    <div className="oc-welcome">
      <div className="oc-welcome-inner">
        <h1>OnCode</h1>
        <div className="oc-muted">A lightweight code editor with a complete Git client.</div>
        <div className="oc-welcome-actions">
          <button className="oc-btn oc-btn-primary" onClick={() => void pickAndOpenFolder()}>
            <FolderOpen size={15} /> Open Folder…
          </button>
          <button className="oc-btn" disabled={!info?.git} onClick={() => void executeCommand("git.clone").catch(showError)}>
            <GitBranch size={15} /> Clone Repository…
          </button>
          <button className="oc-btn" onClick={() => void executeCommand("settings.open").catch(showError)}>
            <Settings size={15} /> Settings
          </button>
        </div>
        {info && !info.git && (
          <div className="oc-banner" style={{ marginTop: 16, borderRadius: 8, border: "1px solid var(--border)" }}>
            <TriangleAlert size={16} />
            <span className="oc-selectable">
              Git features are unavailable: {info.gitError ?? "git was not found"}. Install Git 2.40 or newer and restart OnCode.
            </span>
          </div>
        )}
        {recent.length > 0 && (
          <>
            <h2>Recent</h2>
            {recent.slice(0, 10).map((path) => (
              <button
                key={path}
                className="oc-recent"
                title={path}
                onClick={() => void openFolder(path)}
                onContextMenu={(e) => {
                  e.preventDefault();
                  void commands.recentRemove(path).then(() => commands.appInfo()).then((i) => useApp.setState({ info: i }));
                }}
              >
                <span className="oc-recent-name">{baseName(path)}</span>
                <span className="oc-faint oc-ellipsis">{path}</span>
              </button>
            ))}
          </>
        )}
        <h2>Shortcuts</h2>
        <div className="oc-muted" style={{ lineHeight: 1.9 }}>
          <div><span className="oc-kbd">Ctrl+Shift+P</span> &nbsp;Command palette</div>
          <div><span className="oc-kbd">Ctrl+P</span> &nbsp;Go to file</div>
          <div><span className="oc-kbd">Ctrl+Shift+G</span> &nbsp;Source Control</div>
          <div><span className="oc-kbd">Ctrl+`</span> &nbsp;Terminal</div>
        </div>
        {info && <div className="oc-faint" style={{ marginTop: 22, fontSize: 11 }}>Version {info.version}{info.git ? ` · git ${info.git.version}` : ""}</div>}
      </div>
    </div>
  );
}
