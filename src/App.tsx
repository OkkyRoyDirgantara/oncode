import { Component, useEffect } from "react";
import type { ErrorInfo, ReactNode } from "react";
import { OverlayHost } from "@/app/overlay";
import { useApp } from "@/app/store";
import { Welcome } from "@/app/Welcome";
import { BareWorkbench, Workbench } from "@/app/Workbench";
import { useWorkbench } from "@/app/workbench";

/** Menerapkan tema dan font dari pengaturan ke dokumen. */
function useTheme() {
  const theme = useApp((s) => s.settings?.workbench.theme ?? "system");
  const fontFamily = useApp((s) => s.settings?.editor.fontFamily);
  const fontSize = useApp((s) => s.settings?.editor.fontSize);

  useEffect(() => {
    const media = window.matchMedia("(prefers-color-scheme: light)");
    const apply = () => {
      const resolved = theme === "system" ? (media.matches ? "light" : "dark") : theme;
      document.documentElement.dataset.theme = resolved;
      window.dispatchEvent(new CustomEvent("oncode:theme", { detail: resolved }));
    };
    apply();
    media.addEventListener("change", apply);
    return () => media.removeEventListener("change", apply);
  }, [theme]);

  useEffect(() => {
    const style = document.documentElement.style;
    if (fontFamily) style.setProperty("--editor-font", fontFamily);
    if (fontSize) style.setProperty("--editor-font-size", `${fontSize}px`);
  }, [fontFamily, fontSize]);
}

/** Menampilkan galat render alih-alih layar kosong. */
class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state = { error: null as Error | null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("Render gagal", error, info.componentStack);
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="oc-empty oc-selectable" role="alert">
        <div style={{ maxWidth: 720, textAlign: "left" }}>
          <h2 style={{ margin: "0 0 8px" }}>Something went wrong</h2>
          <pre className="oc-dialog-detail oc-mono" id="oc-render-error">{`${error.message}\n\n${error.stack ?? ""}`}</pre>
          <button className="oc-btn oc-btn-primary" style={{ marginTop: 12 }} onClick={() => window.location.reload()}>
            Reload Window
          </button>
        </div>
      </div>
    );
  }
}

export function App() {
  const ready = useApp((s) => s.ready);
  const workspace = useApp((s) => s.workspace);
  // Tanpa folder, tab yang terbuka (mis. Settings) ditampilkan menggantikan layar awal.
  const hasTabs = useWorkbench((s) => s.groups.some((g) => g.tabs.length > 0));
  useTheme();
  return (
    <ErrorBoundary>
      {!ready ? null : workspace ? <Workbench /> : hasTabs ? <BareWorkbench /> : <Welcome />}
      <OverlayHost />
    </ErrorBoundary>
  );
}
