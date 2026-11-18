// Kerangka tampilan: activity bar, sidebar, grup editor bertab, panel bawah, status bar.
import { useEffect } from "react";
import type { MouseEvent as ReactMouseEvent } from "react";
import { Group as PanelGroup, Panel, Separator, usePanelRef } from "react-resizable-panels";
import { Columns2, PanelBottom, Rows2, Settings as SettingsIcon, ShieldAlert, X } from "lucide-react";
import { executeCommand, formatKeybinding } from "./commands";
import { showMenu } from "./overlay";
import { getPanelViews, getSidebarViews, getStatusItems, getTabType, type PanelView, type SidebarView, type Tab } from "./registry";
import { setTrusted, showSidebar, togglePanel, useApp } from "./store";
import { activateGroup, activateTab, closeTab, closeTabs, pinTab, splitActiveTab, toggleOrientation, useWorkbench, type Group } from "./workbench";

function ActivityButton({ view }: { view: SidebarView }) {
  const active = useApp((s) => s.sidebarVisible && s.sidebarView === view.id);
  const badge = view.useBadge?.();
  const visible = view.useVisible?.() ?? true;
  if (!visible) return null;
  return (
    <button className="oc-activity" title={view.title} aria-label={view.title} aria-pressed={active} onClick={() => showSidebar(view.id)}>
      {view.icon}
      {badge ? <span className="oc-badge">{badge > 99 ? "99+" : badge}</span> : null}
    </button>
  );
}

/** Menampilkan atau menyembunyikan panel bawah (Terminal, Problems, Git Output). */
function PanelToggle() {
  const visible = useApp((s) => s.panelVisible);
  return (
    <button className="oc-activity" title={`${visible ? "Hide" : "Show"} Panel (${formatKeybinding("Mod+J")})`} aria-label="Toggle Panel" aria-pressed={visible} onClick={() => togglePanel()}>
      <PanelBottom size={20} />
    </button>
  );
}

function ActivityBar() {
  return (
    <nav className="oc-activitybar" aria-label="Views">
      {getSidebarViews().map((v) => (
        <ActivityButton key={v.id} view={v} />
      ))}
      <div className="oc-grow" />
      <PanelToggle />
      <button className="oc-activity" title="Settings" aria-label="Settings" onClick={() => void executeCommand("settings.open")}>
        <SettingsIcon size={20} />
      </button>
    </nav>
  );
}

function Sidebar() {
  const requested = useApp((s) => s.sidebarView);
  const views = getSidebarViews();
  // Tampilan yang diminta mungkin tidak terdaftar; pakai yang pertama agar sidebar tidak kosong.
  const viewId = views.some((v) => v.id === requested) ? requested : views[0]?.id;
  return (
    <aside className="oc-sidebar">
      {/* Semua tampilan tetap terpasang agar keadaannya (gulir, isian) bertahan saat berpindah. */}
      {views.map((v) => (
        <div key={v.id} className="oc-col oc-grow" hidden={v.id !== viewId} style={{ height: "100%", display: v.id === viewId ? "flex" : "none" }}>
          <v.component />
        </div>
      ))}
    </aside>
  );
}

function TabButton({ tab, group, selected }: { tab: Tab; group: Group; selected: boolean }) {
  const type = getTabType(tab.kind);
  const dirty = type?.useDirty?.(tab) ?? false;

  useEffect(() => {
    // Tab pratinjau yang mulai disunting menjadi tab tetap.
    if (dirty && tab.preview) pinTab(tab.id);
  }, [dirty, tab.preview, tab.id]);

  const onContextMenu = (e: ReactMouseEvent) => {
    e.preventDefault();
    const others = group.tabs.filter((t) => t.id !== tab.id).map((t) => t.id);
    const index = group.tabs.findIndex((t) => t.id === tab.id);
    const path = typeof tab.params.path === "string" ? tab.params.path : null;
    showMenu(e, [
      { label: "Close", run: () => closeTab(group.id, tab.id) },
      { label: "Close Others", disabled: others.length === 0, run: () => closeTabs(group.id, others) },
      { label: "Close to the Right", disabled: index === group.tabs.length - 1, run: () => closeTabs(group.id, group.tabs.slice(index + 1).map((t) => t.id)) },
      { label: "Close All", run: () => closeTabs(group.id, group.tabs.map((t) => t.id)) },
      { separator: true },
      {
        label: "Split Right",
        run: () => {
          activateTab(group.id, tab.id);
          splitActiveTab();
        },
      },
      { separator: true },
      { label: "Copy Path", disabled: !path, run: () => path && navigator.clipboard.writeText(path) },
    ]);
  };

  return (
    <div
      role="tab"
      tabIndex={-1}
      className="oc-tab"
      aria-selected={selected}
      data-preview={tab.preview || undefined}
      data-dirty={dirty || undefined}
      title={tab.description ?? tab.title}
      onMouseDown={(e) => {
        if (e.button === 0) activateTab(group.id, tab.id);
        if (e.button === 1) {
          e.preventDefault();
          void closeTab(group.id, tab.id);
        }
      }}
      onDoubleClick={() => pinTab(tab.id)}
      onContextMenu={onContextMenu}
    >
      {type?.icon?.(tab)}
      <span className="oc-tab-label oc-ellipsis">{tab.title}</span>
      <button
        className="oc-tab-close"
        aria-label={`Close ${tab.title}`}
        onMouseDown={(e) => e.stopPropagation()}
        onClick={(e) => {
          e.stopPropagation();
          void closeTab(group.id, tab.id);
        }}
      >
        {dirty ? <span className="oc-tab-dirty" /> : <X size={14} />}
      </button>
    </div>
  );
}

function EditorGroup({ group }: { group: Group }) {
  const isActiveGroup = useWorkbench((s) => s.activeGroupId === group.id);
  const orientation = useWorkbench((s) => s.orientation);
  return (
    <section className="oc-group" data-active={isActiveGroup} onMouseDownCapture={() => activateGroup(group.id)} onFocusCapture={() => activateGroup(group.id)}>
      <div className="oc-tabs" role="tablist">
        {group.tabs.map((tab) => (
          <TabButton key={tab.id} tab={tab} group={group} selected={tab.id === group.activeId} />
        ))}
        <div className="oc-tab-actions">
          <button className="oc-iconbtn" title="Split editor" aria-label="Split editor" disabled={!group.activeId} onClick={() => splitActiveTab()}>
            <Columns2 size={15} />
          </button>
          <button className="oc-iconbtn" title="Toggle split direction" aria-label="Toggle split direction" onClick={() => toggleOrientation()}>
            {orientation === "horizontal" ? <Rows2 size={15} /> : <Columns2 size={15} />}
          </button>
        </div>
      </div>
      <div className="oc-group-body">
        {group.tabs.length === 0 && <div className="oc-empty">No editor open</div>}
        {group.tabs.map((tab) => {
          const type = getTabType(tab.kind);
          const visible = tab.id === group.activeId;
          if (!type) return visible ? <div key={tab.id} className="oc-empty">Unknown tab type: {tab.kind}</div> : null;
          if (!visible && type.keepAlive === false) return null;
          const Component = type.component;
          return (
            <div key={tab.id} className="oc-tabpane" hidden={!visible} role="tabpanel">
              <Component tab={tab} groupId={group.id} visible={visible} focused={visible && isActiveGroup} />
            </div>
          );
        })}
      </div>
    </section>
  );
}

function EditorArea() {
  const groups = useWorkbench((s) => s.groups);
  const orientation = useWorkbench((s) => s.orientation);
  if (groups.length === 1) return <EditorGroup group={groups[0]} />;
  return (
    <PanelGroup orientation={orientation} id="editor-groups" key={`${orientation}:${groups.map((g) => g.id).join(",")}`}>
      {groups.flatMap((group, i) => {
        const panel = (
          <Panel key={group.id} id={group.id} minSize={120}>
            <EditorGroup group={group} />
          </Panel>
        );
        if (i === 0) return [panel];
        return [<Separator key={`sep-${group.id}`} className={`oc-separator ${orientation === "horizontal" ? "oc-separator-h" : "oc-separator-v"}`} />, panel];
      })}
    </PanelGroup>
  );
}

function PanelTab({ view, selected }: { view: PanelView; selected: boolean }) {
  const badge = view.useBadge?.();
  return (
    <button role="tab" className="oc-panel-tab" aria-selected={selected} onClick={() => useApp.setState({ panelView: view.id, panelVisible: true })}>
      {view.title}
      {badge ? <span className="oc-count" style={{ marginLeft: 6 }}>{badge}</span> : null}
    </button>
  );
}

function BottomPanel() {
  const viewId = useApp((s) => s.panelView);
  const shown = useApp((s) => s.panelVisible);
  const views = getPanelViews();
  const current = views.find((v) => v.id === viewId) ?? views[0];
  return (
    <div className="oc-panel">
      <div className="oc-panel-tabs" role="tablist">
        {views.map((v) => (
          <PanelTab key={v.id} view={v} selected={v.id === current?.id} />
        ))}
        <div className="oc-grow" />
        {current?.actions && <current.actions />}
        <button className="oc-iconbtn" title="Hide panel" aria-label="Hide panel" onClick={() => togglePanel()}>
          <X size={15} />
        </button>
      </div>
      <div className="oc-panel-body">
        {views.map((v) => (
          <div key={v.id} className="oc-tabpane" hidden={v.id !== current?.id}>
            <v.component visible={shown && v.id === current?.id} />
          </div>
        ))}
      </div>
    </div>
  );
}

function StatusBar() {
  return (
    <footer className="oc-statusbar">
      {getStatusItems("left").map((i) => (
        <i.component key={i.id} />
      ))}
      <div className="oc-grow" />
      {getStatusItems("right").map((i) => (
        <i.component key={i.id} />
      ))}
    </footer>
  );
}

function TrustBanner() {
  const workspace = useApp((s) => s.workspace);
  if (!workspace || workspace.trusted) return null;
  return (
    <div className="oc-banner" role="status">
      <ShieldAlert size={16} />
      <span>
        <strong>Restricted mode.</strong> Git, language servers and project settings are disabled until you trust the authors of the files in this folder.
      </span>
      <div className="oc-banner-actions">
        <button className="oc-btn oc-btn-sm oc-btn-primary" onClick={() => void setTrusted(true)}>
          Trust folder
        </button>
      </div>
    </div>
  );
}

/** Kerangka tanpa folder: hanya area editor, untuk tab seperti Settings yang dibuka dari layar awal. */
export function BareWorkbench() {
  return (
    <div className="oc-app">
      <div className="oc-main">
        <div className="oc-editor-area oc-grow">
          <EditorArea />
        </div>
      </div>
      <StatusBar />
    </div>
  );
}

export function Workbench() {
  const sidebarVisible = useApp((s) => s.sidebarVisible);
  const panelVisible = useApp((s) => s.panelVisible);
  const sidebarRef = usePanelRef();
  const panelRef = usePanelRef();

  // Sidebar dan panel bawah tetap terpasang (terminal dan gulir bertahan); menyembunyikan = menciutkan.
  useEffect(() => {
    const panel = sidebarRef.current;
    if (!panel) return;
    if (sidebarVisible && panel.isCollapsed()) panel.expand();
    if (!sidebarVisible && !panel.isCollapsed()) panel.collapse();
  }, [sidebarVisible, sidebarRef]);
  useEffect(() => {
    const panel = panelRef.current;
    if (!panel) return;
    if (panelVisible && panel.isCollapsed()) panel.expand();
    if (!panelVisible && !panel.isCollapsed()) panel.collapse();
  }, [panelVisible, panelRef]);

  return (
    <div className="oc-app">
      <TrustBanner />
      <div className="oc-main">
        <ActivityBar />
        <PanelGroup orientation="horizontal" id="main">
          <Panel
            id="sidebar"
            panelRef={sidebarRef}
            defaultSize={300}
            minSize={200}
            maxSize={720}
            collapsible
            collapsedSize={0}
            groupResizeBehavior="preserve-pixel-size"
            onResize={(size) => {
              const collapsed = size.inPixels < 2;
              if (collapsed === useApp.getState().sidebarVisible) useApp.setState({ sidebarVisible: !collapsed });
            }}
          >
            <Sidebar />
          </Panel>
          <Separator className="oc-separator oc-separator-h" />
          <Panel id="center" minSize={240}>
            <PanelGroup orientation="vertical" id="center-split">
              <Panel id="editors" minSize={80}>
                <div className="oc-editor-area">
                  <EditorArea />
                </div>
              </Panel>
              <Separator className="oc-separator oc-separator-v" />
              <Panel
                id="bottom"
                panelRef={panelRef}
                defaultSize={0}
                minSize={110}
                collapsible
                collapsedSize={0}
                groupResizeBehavior="preserve-pixel-size"
                onResize={(size) => {
                  const collapsed = size.inPixels < 2;
                  if (collapsed === useApp.getState().panelVisible) useApp.setState({ panelVisible: !collapsed });
                }}
              >
                <BottomPanel />
              </Panel>
            </PanelGroup>
          </Panel>
        </PanelGroup>
      </div>
      <StatusBar />
    </div>
  );
}
