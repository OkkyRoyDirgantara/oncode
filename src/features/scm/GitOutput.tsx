// Panel Git Output: setiap perintah git yang dijalankan aplikasi beserta keluarannya.
import { useEffect, useRef } from "react";
import { Trash2 } from "lucide-react";
import { create } from "zustand";
import { events, type GitLogEntry } from "@/ipc";

const MAX_ENTRIES = 500;

interface Entry extends GitLogEntry {
  id: number;
  time: number;
}

const useGitOutput = create<{ entries: Entry[] }>(() => ({ entries: [] }));
let nextId = 1;

export function installGitOutput(): void {
  void events.gitOutput.listen((e) => {
    useGitOutput.setState((s) => ({ entries: [...s.entries.slice(-(MAX_ENTRIES - 1)), { ...e.payload.entry, id: nextId++, time: Date.now() }] }));
  });
}

export function GitOutputActions() {
  return (
    <button className="oc-iconbtn" title="Clear output" aria-label="Clear output" onClick={() => useGitOutput.setState({ entries: [] })}>
      <Trash2 size={14} />
    </button>
  );
}

export function GitOutputPanel({ visible }: { visible: boolean }) {
  const entries = useGitOutput((s) => s.entries);
  const end = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (visible) end.current?.scrollIntoView({ block: "end" });
  }, [entries, visible]);
  if (entries.length === 0) return <div className="oc-empty">Git commands run by OnCode appear here.</div>;
  return (
    <div className="gitout oc-mono oc-selectable">
      {entries.map((entry) => (
        <div key={entry.id} className="gitout-entry">
          <div>
            <span className="gitout-meta">{new Date(entry.time).toLocaleTimeString()} </span>
            <span className="gitout-cmd" data-failed={entry.exitCode !== 0 || undefined}>&gt; {entry.command}</span>
            <span className="gitout-meta">
              {" "}
              [{entry.durationMs} ms{entry.exitCode !== 0 ? `, exit ${entry.exitCode}` : ""}]
            </span>
          </div>
          {entry.output && <pre className="gitout-output oc-mono">{entry.output}</pre>}
        </div>
      ))}
      <div ref={end} />
    </div>
  );
}
