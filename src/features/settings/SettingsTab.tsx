// Halaman pengaturan: formulir di atas pengaturan tingkat PENGGUNA. Setiap perubahan langsung disimpan
// (isian teks di-debounce); nilai yang tidak sah tidak pernah disimpan.
import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { Check, FileJson, Keyboard, RotateCcw, Search, X } from "lucide-react";
import { executeCommand } from "@/app/commands";
import { openFile } from "@/app/coreCommands";
import { confirm, showError } from "@/app/overlay";
import type { TabProps } from "@/app/registry";
import { useApp } from "@/app/store";
import { commands, errorMessage, events, type LspServerInfo, type ServerSpec, type Settings } from "@/ipc";
import {
  FIELDS,
  SECTIONS,
  applyDefault,
  countNonDefault,
  defaultLabel,
  differsFromDefault,
  fieldMatches,
  fieldText,
  isOverridden,
  isServerOverridden,
  joinArgs,
  matchesQuery,
  parseNumber,
  sameValue,
  splitArgs,
  validateServerCommand,
  withServerOverride,
  type Field,
  type OverrideContext,
  type Parsed,
} from "./model";
import "./settings.css";

/** Jeda sebelum isian teks disimpan. */
export const TEXT_DEBOUNCE_MS = 400;

type Commit = (change: (settings: Settings) => Settings) => void;

function OverrideMarker() {
  return (
    <span className="st-override" title="A project setting in .oncode/settings.json replaces this value while this folder is open. Changing it here affects other folders.">
      overridden by project (.oncode/settings.json)
    </span>
  );
}

interface DebouncedInputProps<T> {
  id: string;
  /** Nilai tersimpan, sebagai teks. */
  value: string;
  parse: (text: string) => Parsed<T>;
  onCommit: (value: T) => void;
  placeholder?: string;
  monospace?: boolean;
  numeric?: boolean;
  describedBy?: string;
  label?: string;
  className?: string;
  /** Berubah saat kolom di-reset: suntingan lokal (termasuk isian yang tidak sah) dibuang. */
  resetSignal?: number;
}

/**
 * Kotak isian yang menyimpan setelah jeda singkat. Selama pengguna mengetik (fokus atau ada
 * suntingan yang belum tersimpan), nilai dari luar tidak menimpa isinya.
 */
function DebouncedInput<T>({ id, value, parse, onCommit, placeholder, monospace, numeric, describedBy, label, className, resetSignal = 0 }: DebouncedInputProps<T>) {
  const [text, setText] = useState(value);
  const [focused, setFocused] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const pending = useRef<{ value: T } | null>(null);
  const commitRef = useRef(onCommit);
  commitRef.current = onCommit;
  const errorId = `${id}-error`;

  // Nilai dari luar (jendela lain, berkas disunting) hanya diambil bila kolom ini tidak sedang disunting.
  useEffect(() => {
    if (!focused && !dirty) setText(value);
  }, [value, focused, dirty]);

  // Reset: apa pun yang sedang diketik di sini kalah oleh nilai yang baru ditetapkan.
  const seenReset = useRef(resetSignal);
  useEffect(() => {
    if (seenReset.current === resetSignal) return;
    seenReset.current = resetSignal;
    clearTimeout(timer.current);
    timer.current = undefined;
    pending.current = null;
    setProblem(null);
    setDirty(false);
    setText(value);
    // Hanya bereaksi pada sinyal reset; `value` diambil dari render yang sama.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [resetSignal]);

  const flush = useCallback(() => {
    clearTimeout(timer.current);
    timer.current = undefined;
    const ready = pending.current;
    if (!ready) return;
    pending.current = null;
    commitRef.current(ready.value);
    setDirty(false);
  }, []);

  // Suntingan yang masih menunggu ikut disimpan saat halaman ditutup.
  useEffect(
    () => () => {
      clearTimeout(timer.current);
      const ready = pending.current;
      pending.current = null;
      if (ready) commitRef.current(ready.value);
    },
    [],
  );

  const change = (next: string) => {
    setText(next);
    setDirty(true);
    clearTimeout(timer.current);
    const parsed = parse(next);
    if (!parsed.ok) {
      pending.current = null;
      setProblem(parsed.message);
      return;
    }
    setProblem(null);
    pending.current = { value: parsed.value };
    timer.current = setTimeout(flush, TEXT_DEBOUNCE_MS);
  };

  return (
    <div className={`st-input-wrap${className ? ` ${className}` : ""}`}>
      <input
        id={id}
        className={`oc-input${monospace ? " oc-mono" : ""}${numeric ? " st-number" : ""}`}
        type="text"
        inputMode={numeric ? "numeric" : undefined}
        value={text}
        placeholder={placeholder}
        spellCheck={false}
        autoComplete="off"
        aria-label={label}
        aria-invalid={problem ? true : undefined}
        aria-describedby={[describedBy, problem ? errorId : null].filter(Boolean).join(" ") || undefined}
        onChange={(e) => change(e.target.value)}
        onFocus={() => setFocused(true)}
        onBlur={() => {
          setFocused(false);
          flush();
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") flush();
        }}
      />
      {problem && (
        <div id={errorId} className="oc-error-text" role="alert">
          {problem} — not saved
        </div>
      )}
    </div>
  );
}

interface FieldRowProps {
  field: Field;
  user: Settings;
  overridden: boolean;
  commit: Commit;
  /** Nilai bawaan; null bila belum (atau gagal) dimuat, sehingga tombol reset tidak ditampilkan. */
  defaults: Settings | null;
  resetSignal: number;
  onReset: (field: Field) => void;
}

function FieldRow({ field, user, overridden, commit, defaults, resetSignal, onReset }: FieldRowProps) {
  const id = `st-${field.id}`;
  const descriptionId = `${id}-description`;
  let control: ReactNode;
  switch (field.kind) {
    case "checkbox":
      control = <input id={id} type="checkbox" checked={field.get(user)} aria-describedby={descriptionId} onChange={(e) => commit((s) => field.set(s, e.target.checked))} />;
      break;
    case "select":
      control = (
        <select id={id} className="oc-select st-select" value={field.get(user)} aria-describedby={descriptionId} onChange={(e) => commit((s) => field.set(s, e.target.value))}>
          {field.options.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
      );
      break;
    case "number":
      control = (
        <div className="st-with-unit">
          <DebouncedInput<number | null>
            id={id}
            value={fieldText(field, user)}
            parse={(text) => parseNumber(field, text)}
            onCommit={(value) => commit((s) => field.set(s, value))}
            placeholder={field.placeholder}
            numeric
            describedBy={descriptionId}
            resetSignal={resetSignal}
          />
          {field.unit && <span className="oc-faint st-unit">{field.unit}</span>}
        </div>
      );
      break;
    case "text":
      control = (
        <DebouncedInput<string>
          id={id}
          value={fieldText(field, user)}
          parse={(text) => {
            const message = field.validate?.(text) ?? null;
            return message ? { ok: false, message } : { ok: true, value: text };
          }}
          onCommit={(value) => commit((s) => field.set(s, value))}
          placeholder={field.placeholder}
          monospace={field.monospace}
          describedBy={descriptionId}
          className="st-wide"
          resetSignal={resetSignal}
        />
      );
      break;
  }
  return (
    <div className="st-row" data-setting={field.id} data-kind={field.kind}>
      <div className="st-row-text">
        <label className="st-label" htmlFor={id}>
          {field.label}
        </label>
        {overridden && <OverrideMarker />}
        <div id={descriptionId} className="st-description">
          {field.description}
        </div>
      </div>
      <div className="st-row-control">
        {control}
        {defaults && differsFromDefault(field, user, defaults) && (
          <button className="oc-iconbtn st-reset" title={`Reset to default (${defaultLabel(field, defaults)})`} aria-label={`Reset ${field.label} to default`} onClick={() => onReset(field)}>
            <RotateCcw size={13} />
          </button>
        )}
      </div>
    </div>
  );
}

function ServerRow({ info, override, overridden, commit }: { info: LspServerInfo; override: ServerSpec | undefined; overridden: boolean; commit: Commit }) {
  const base = useId();
  const language = info.language;
  // Penimpaan selalu dibangun dari nilai terbaru di pengaturan, agar `env` dan kolom lain tidak hilang.
  const update = (patch: Partial<ServerSpec>) => commit((s) => withServerOverride(s, language, { ...(s.lsp.servers[language] ?? { command: info.command, args: info.args }), ...patch }));
  return (
    <tr data-language={language}>
      <td className="st-server-language">
        <div>{language}</div>
        {overridden && <OverrideMarker />}
      </td>
      <td>
        <code className="st-code oc-selectable">{[info.command, joinArgs(info.args)].filter(Boolean).join(" ")}</code>
      </td>
      <td className="st-server-status">
        {info.installed ? (
          <span className="st-installed" title="Found on PATH">
            <Check size={13} aria-hidden="true" /> Installed
          </span>
        ) : (
          <>
            <span className="st-missing" title={info.installHint || "Not found on PATH"}>
              <X size={13} aria-hidden="true" /> Not found
            </span>
            {info.installHint && <div className="st-hint oc-selectable">{info.installHint}</div>}
          </>
        )}
      </td>
      <td className="st-server-override">
        {override ? (
          <div className="st-server-edit">
            <DebouncedInput<string>
              id={`${base}-command`}
              label={`Command for ${language}`}
              value={override.command}
              parse={(text) => {
                const message = validateServerCommand(text);
                return message ? { ok: false, message } : { ok: true, value: text.trim() };
              }}
              onCommit={(command) => update({ command })}
              placeholder="Command"
              monospace
            />
            <DebouncedInput<string[]>
              id={`${base}-args`}
              label={`Arguments for ${language}`}
              value={joinArgs(override.args ?? [])}
              parse={splitArgs}
              onCommit={(args) => update({ args })}
              placeholder="Arguments"
              monospace
            />
            <button className="oc-btn oc-btn-sm" title="Remove this override and use the built-in server" onClick={() => commit((s) => withServerOverride(s, language, null))}>
              Reset
            </button>
          </div>
        ) : (
          <button className="oc-btn oc-btn-sm" title="Use a different command or arguments for this language" onClick={() => update({})}>
            Customize
          </button>
        )}
      </td>
    </tr>
  );
}

const SERVER_TABLE_KEYWORDS = "language servers lsp table command arguments override installed customize reset";
const KEYBOARD_KEYWORDS = "keyboard shortcuts keybindings keybindings.json keys hotkeys open";

export function SettingsPage() {
  const info = useApp((s) => s.info);
  const workspace = useApp((s) => s.workspace);
  const effective = useApp((s) => s.settings);
  const [user, setUser] = useState<Settings | null>(null);
  /** Pengaturan pengguna yang terakhir diketahui tersimpan; pembanding untuk penanda "ditimpa proyek". */
  const [saved, setSaved] = useState<Settings | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [servers, setServers] = useState<LspServerInfo[] | null>(null);
  /** Nilai bawaan dari backend, untuk "Reset to defaults" dan tombol reset per kolom. */
  const [defaults, setDefaults] = useState<Settings | null>(null);
  const [resets, setResets] = useState<{ all: number; fields: Record<string, number> }>({ all: 0, fields: {} });
  const latest = useRef<Settings | null>(null);
  const revision = useRef(0);
  const chain = useRef<Promise<void>>(Promise.resolve());
  const alive = useRef(true);

  /** Mengambil pengaturan pengguna dari backend, kecuali ada perubahan lokal yang lebih baru. */
  const sync = useCallback(async () => {
    const startedAt = revision.current;
    try {
      // Penyimpanan yang sedang berjalan diselesaikan dulu agar yang terbaca bukan keadaan lama.
      await chain.current;
      const incoming = await commands.settingsGetUser();
      if (!alive.current || revision.current !== startedAt) return;
      latest.current = incoming;
      setUser(incoming);
      setSaved(incoming);
      setLoadError(null);
    } catch (e) {
      if (alive.current && !latest.current) setLoadError(errorMessage(e));
    }
  }, []);

  /** Menerapkan perubahan ke pengaturan terbaru lalu menyimpannya. Penyimpanan berurutan, tidak saling mendahului. */
  const commit = useCallback<Commit>(
    (change) => {
      const base = latest.current;
      if (!base) return;
      const next = change(base);
      if (sameValue(next, base)) return;
      latest.current = next;
      revision.current += 1;
      if (alive.current) setUser(next);
      chain.current = chain.current.then(async () => {
        try {
          await commands.settingsSet(next);
          if (alive.current) setSaved(next);
        } catch (e) {
          showError(e, "Could not save settings");
          // Tampilan dikembalikan ke apa yang benar-benar tersimpan.
          void Promise.resolve().then(sync);
        }
      });
    },
    [sync],
  );

  useEffect(() => {
    alive.current = true;
    void sync();
    // Jendela lain (atau penyimpanan dari sini) mengubah pengaturan: ambil ulang pengaturan pengguna.
    // Payload event berisi pengaturan EFEKTIF jendela ini, jadi tidak dipakai langsung.
    const unlisten = events.settingsChanged.listen(() => void sync());
    // Tanpa nilai bawaan, halaman tetap berfungsi; hanya tombol reset yang tidak muncul.
    commands.settingsDefaults().then(
      (value) => {
        if (alive.current) setDefaults(value);
      },
      () => undefined,
    );
    return () => {
      alive.current = false;
      void unlisten.then((stop) => stop()).catch(() => undefined);
    };
  }, [sync]);

  // Tabel server dimuat ulang setiap kali penimpaan server atau sakelar LSP berubah (status terpasang ikut berubah).
  const serversKey = user ? JSON.stringify([user.lsp.servers, user.lsp.enabled]) : null;
  useEffect(() => {
    if (serversKey === null) return;
    let cancelled = false;
    void chain.current
      .then(() => commands.lspServers())
      .then(
        (list) => {
          if (!cancelled) setServers(list);
        },
        () => {
          if (!cancelled) setServers((current) => current ?? []);
        },
      );
    return () => {
      cancelled = true;
    };
  }, [serversKey]);

  const context: OverrideContext | null = useMemo(
    () => (saved ? { trusted: !!workspace?.trusted, user: saved, effective } : null),
    [saved, workspace?.trusted, effective],
  );

  const resetField = (field: Field) => {
    if (!defaults) return;
    commit((s) => applyDefault(field, s, defaults));
    setResets((r) => ({ ...r, fields: { ...r.fields, [field.id]: (r.fields[field.id] ?? 0) + 1 } }));
  };

  const resetAll = async () => {
    if (!defaults || !latest.current) return;
    const count = countNonDefault(latest.current, defaults);
    const ok = await confirm({
      title: "Reset all settings to their defaults?",
      message: `${count} setting${count === 1 ? "" : "s"} you changed will return to ${count === 1 ? "its default" : "their defaults"}, including language server overrides and OAuth Client IDs. This cannot be undone.\n\nSigned-in accounts, keyboard shortcuts and project settings are not affected.`,
      confirmLabel: "Reset Settings",
      danger: true,
    });
    if (!ok || !alive.current) return;
    commit(() => defaults);
    setResets((r) => ({ ...r, all: r.all + 1 }));
  };

  if (!user || !context) {
    return (
      <div className="st-page">
        <div className="st-inner">
          <h1 className="st-title">Settings</h1>
          {loadError ? (
            <div role="alert">
              <p className="oc-selectable">Could not load settings: {loadError}</p>
              <button className="oc-btn" onClick={() => void sync()}>Retry</button>
            </div>
          ) : (
            <p className="oc-muted">Loading…</p>
          )}
        </div>
      </div>
    );
  }

  const tableMatches = matchesQuery(query, SERVER_TABLE_KEYWORDS);
  const visibleServers = (servers ?? []).filter((s) => tableMatches || matchesQuery(query, s.language, s.command, joinArgs(s.args)));
  const keyboardMatches = matchesQuery(query, KEYBOARD_KEYWORDS);
  const changedCount = defaults ? countNonDefault(user, defaults) : 0;
  let anyVisible = false;

  const sections = SECTIONS.map((section) => {
    const fields = FIELDS.filter((f) => f.section === section.id && fieldMatches(f, query));
    const showTable = section.id === "lsp" && (tableMatches || visibleServers.length > 0);
    const showKeyboard = section.id === "keyboard" && keyboardMatches;
    if (fields.length === 0 && !showTable && !showKeyboard) return null;
    anyVisible = true;
    return (
      <section key={section.id} className="st-section" aria-labelledby={`st-section-${section.id}`}>
        <h2 id={`st-section-${section.id}`} className="st-section-title">
          {section.title}
        </h2>
        {fields.map((field) => (
          <FieldRow
            key={field.id}
            field={field}
            user={user}
            overridden={isOverridden(field, context)}
            commit={commit}
            defaults={defaults}
            resetSignal={resets.all + (resets.fields[field.id] ?? 0)}
            onReset={resetField}
          />
        ))}
        {showTable && (
          <div className="st-servers" data-setting="lsp.servers">
            {servers === null ? (
              <div className="oc-muted">Loading language servers…</div>
            ) : (
              <table className="st-table">
                <thead>
                  <tr>
                    <th>Language</th>
                    <th>Command</th>
                    <th>Status</th>
                    <th>Your override</th>
                  </tr>
                </thead>
                <tbody>
                  {visibleServers.map((s) => (
                    <ServerRow key={s.language} info={s} override={user.lsp.servers[s.language]} overridden={isServerOverridden(s.language, context)} commit={commit} />
                  ))}
                </tbody>
              </table>
            )}
            <div className="st-description st-servers-note">
              Language servers are not bundled with OnCode. Install the ones you need and make sure they are on PATH, or point a language to a specific program with an override. Other languages can be added under <code className="st-code">lsp.servers</code> in settings.json.
            </div>
          </div>
        )}
        {showKeyboard && (
          <div className="st-row" data-setting="keybindings">
            <div className="st-row-text">
              <div className="st-label">Keyboard shortcuts</div>
              <div className="st-description">
                Shortcuts are customised in keybindings.json, an object that maps a command id to a shortcut such as <code className="st-code">"Mod+Shift+K"</code> (or a list of them). Saving the file applies it.
              </div>
            </div>
            <div className="st-row-control">
              <button className="oc-btn" title={info?.keybindingsPath} onClick={() => void executeCommand("keybindings.openJson").catch((e) => showError(e, "Could not open keybindings.json"))}>
                <Keyboard size={14} /> Open keybindings.json
              </button>
            </div>
          </div>
        )}
      </section>
    );
  });

  return (
    <div className="st-page">
      <div className="st-inner">
        <header className="st-header">
          <h1 className="st-title">Settings</h1>
          <div className="st-header-row">
            <div className="st-search">
              <Search size={14} className="st-search-icon" aria-hidden="true" />
              <input className="oc-input" type="search" placeholder="Search settings" aria-label="Search settings" value={query} spellCheck={false} onChange={(e) => setQuery(e.target.value)} />
            </div>
            <button className="oc-btn" disabled={!info?.settingsPath} title={info?.settingsPath} onClick={() => info?.settingsPath && openFile(info.settingsPath)}>
              <FileJson size={14} /> Open settings.json
            </button>
            {defaults && (
              <button className="oc-btn" disabled={changedCount === 0} title={changedCount === 0 ? "Every setting already has its default value" : `Reset ${changedCount} changed setting${changedCount === 1 ? "" : "s"}`} onClick={() => void resetAll()}>
                <RotateCcw size={14} /> Reset to defaults
              </button>
            )}
          </div>
          <div className="st-description">
            These are your user settings; changes are saved automatically and apply to every folder.
            {workspace?.trusted ? " A project can override them in .oncode/settings.json." : workspace ? " Project settings are ignored in restricted mode." : ""}
          </div>
        </header>
        {sections}
        {!anyVisible && (
          <p className="oc-muted" role="status">
            No settings match “{query}”.
          </p>
        )}
      </div>
    </div>
  );
}

export function SettingsTab(_props: TabProps) {
  return <SettingsPage />;
}
