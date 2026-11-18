// Layanan lapisan atas: dialog, masukan, quick pick, menu konteks, dan toast.
// Fitur memanggil fungsi di sini (mis. `confirm`, `toast.error`) tanpa perlu merender apa pun sendiri.
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent, ReactNode } from "react";
import { create } from "zustand";
import { X } from "lucide-react";
import { errorDetail, errorMessage } from "@/ipc";

// ----- Model -----

export interface ChoiceButton {
  id: string;
  label: string;
  kind?: "primary" | "danger" | "default";
}

export interface MenuItem {
  label?: string;
  run?: () => unknown;
  danger?: boolean;
  disabled?: boolean;
  /** Teks kecil di sisi kanan (mis. keybinding). */
  hint?: string;
  separator?: boolean;
}

export interface ToastAction {
  label: string;
  run: () => unknown;
}

interface Toast {
  id: number;
  kind: "info" | "success" | "warning" | "error";
  message: string;
  actions: ToastAction[];
}

interface DialogEntry {
  id: number;
  wide: boolean;
  /** Tutup saat Escape atau klik di luar. */
  dismissible: boolean;
  render: (close: (value: unknown) => void) => ReactNode;
  resolve: (value: unknown) => void;
  bare: boolean;
}

interface MenuState {
  x: number;
  y: number;
  items: MenuItem[];
}

interface OverlayStore {
  dialogs: DialogEntry[];
  toasts: Toast[];
  menu: MenuState | null;
}

const useOverlay = create<OverlayStore>(() => ({ dialogs: [], toasts: [], menu: null }));
let nextId = 1;

// ----- API -----

/** Membuka dialog dengan isi bebas. Promise selesai dengan nilai yang diberikan ke `close` (null bila dibatalkan). */
export function openDialog<T>(
  render: (close: (value: T | null) => void) => ReactNode,
  options: { wide?: boolean; dismissible?: boolean; bare?: boolean } = {},
): Promise<T | null> {
  return new Promise((resolve) => {
    const entry: DialogEntry = {
      id: nextId++,
      wide: options.wide ?? false,
      dismissible: options.dismissible ?? true,
      bare: options.bare ?? false,
      render: render as DialogEntry["render"],
      resolve: resolve as DialogEntry["resolve"],
    };
    useOverlay.setState((s) => ({ dialogs: [...s.dialogs, entry] }));
  });
}

function closeDialog(id: number, value: unknown) {
  const entry = useOverlay.getState().dialogs.find((d) => d.id === id);
  if (!entry) return;
  useOverlay.setState((s) => ({ dialogs: s.dialogs.filter((d) => d.id !== id) }));
  entry.resolve(value);
}

export interface ChooseOptions {
  title: string;
  message?: string;
  /** Teks panjang (mis. keluaran git) dalam kotak yang bisa digulir. */
  detail?: string;
  buttons: ChoiceButton[];
}

/** Dialog dengan beberapa tombol; mengembalikan id tombol atau null bila dibatalkan. */
export function choose(options: ChooseOptions): Promise<string | null> {
  return openDialog<string>((close) => <ChoiceDialog options={options} close={close} />);
}

export interface ConfirmOptions {
  title: string;
  message?: string;
  detail?: string;
  confirmLabel?: string;
  cancelLabel?: string;
  /** Aksi destruktif: tombol konfirmasi berwarna merah. */
  danger?: boolean;
}

export async function confirm(options: ConfirmOptions): Promise<boolean> {
  const answer = await choose({
    title: options.title,
    message: options.message,
    detail: options.detail,
    buttons: [
      { id: "cancel", label: options.cancelLabel ?? "Cancel" },
      { id: "ok", label: options.confirmLabel ?? "OK", kind: options.danger ? "danger" : "primary" },
    ],
  });
  return answer === "ok";
}

export interface InputOptions {
  title: string;
  message?: string;
  label?: string;
  value?: string;
  placeholder?: string;
  confirmLabel?: string;
  multiline?: boolean;
  password?: boolean;
  /** Mengembalikan pesan galat, atau null/undefined bila sah. */
  validate?: (value: string) => string | null | undefined;
  /** Pilih bagian ini dari nilai awal (mis. nama berkas tanpa ekstensi). */
  selection?: [number, number];
  allowEmpty?: boolean;
}

/** Meminta satu teks dari pengguna; null bila dibatalkan. */
export function promptInput(options: InputOptions): Promise<string | null> {
  return openDialog<string>((close) => <InputDialog options={options} close={close} />);
}

export interface PickItem<T> {
  label: string;
  description?: string;
  value: T;
  /** Indeks karakter `label` yang cocok, untuk disorot. */
  matches?: number[];
  hint?: string;
}

export interface PickOptions<T> {
  placeholder?: string;
  /** Daftar tetap yang disaring di sini, atau fungsi (mis. pencarian di backend). */
  items: PickItem<T>[] | ((query: string) => PickItem<T>[] | Promise<PickItem<T>[]>);
  initialQuery?: string;
  emptyText?: string;
}

/** Daftar pilih dengan penyaring, seperti command palette. */
export function quickPick<T>(options: PickOptions<T>): Promise<T | null> {
  return openDialog<T>((close) => <QuickPick options={options} close={close} />, { bare: true });
}

function pushToast(kind: Toast["kind"], message: string, actions: ToastAction[] = [], timeoutMs?: number) {
  const id = nextId++;
  useOverlay.setState((s) => ({ toasts: [...s.toasts.slice(-4), { id, kind, message, actions }] }));
  const timeout = timeoutMs ?? (kind === "error" || actions.length > 0 ? 12000 : 4500);
  if (timeout > 0) setTimeout(() => dismissToast(id), timeout);
  return id;
}

function dismissToast(id: number) {
  useOverlay.setState((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) }));
}

export const toast = {
  info: (message: string, actions?: ToastAction[]) => pushToast("info", message, actions),
  success: (message: string, actions?: ToastAction[]) => pushToast("success", message, actions),
  warning: (message: string, actions?: ToastAction[]) => pushToast("warning", message, actions),
  error: (message: string, actions?: ToastAction[]) => pushToast("error", message, actions),
};

/** Menampilkan galat sebagai toast; rincian (mis. stderr git) bisa dibuka lewat tombol Details. */
export function showError(error: unknown, context?: string): void {
  const message = errorMessage(error);
  const detail = errorDetail(error);
  const text = context ? `${context}: ${message}` : message;
  const actions: ToastAction[] = detail
    ? [{ label: "Details", run: () => void choose({ title: context ?? "Error", message, detail, buttons: [{ id: "ok", label: "Close", kind: "primary" }] }) }]
    : [];
  pushToast("error", text, actions);
}

/** Membuka menu konteks di posisi penunjuk. */
export function showMenu(position: { clientX: number; clientY: number } | { x: number; y: number }, items: MenuItem[]): void {
  const x = "clientX" in position ? position.clientX : position.x;
  const y = "clientX" in position ? position.clientY : position.y;
  // Buang pemisah ganda dan pemisah di ujung.
  const cleaned: MenuItem[] = [];
  for (const item of items) {
    if (item.separator && (cleaned.length === 0 || cleaned[cleaned.length - 1].separator)) continue;
    cleaned.push(item);
  }
  while (cleaned.length && cleaned[cleaned.length - 1].separator) cleaned.pop();
  if (cleaned.length) useOverlay.setState({ menu: { x, y, items: cleaned } });
}

export function hideMenu(): void {
  if (useOverlay.getState().menu) useOverlay.setState({ menu: null });
}

// ----- Komponen -----

function ChoiceDialog({ options, close }: { options: ChooseOptions; close: (v: string | null) => void }) {
  const primary = useRef<HTMLButtonElement>(null);
  useEffect(() => primary.current?.focus(), []);
  const focusIndex = Math.max(
    options.buttons.findIndex((b) => b.kind === "primary"),
    options.buttons.findIndex((b) => b.kind === "danger") >= 0 ? 0 : -1,
  );
  return (
    <>
      <div className="oc-dialog-header">{options.title}</div>
      <div className="oc-dialog-body">
        {options.message && <p>{options.message}</p>}
        {options.detail && <pre className="oc-dialog-detail oc-mono">{options.detail}</pre>}
      </div>
      <div className="oc-dialog-footer">
        {options.buttons.map((b, i) => (
          <button
            key={b.id}
            ref={i === (focusIndex >= 0 ? focusIndex : options.buttons.length - 1) ? primary : undefined}
            className={`oc-btn ${b.kind === "primary" ? "oc-btn-primary" : b.kind === "danger" ? "oc-btn-danger" : ""}`}
            onClick={() => close(b.id)}
          >
            {b.label}
          </button>
        ))}
      </div>
    </>
  );
}

function InputDialog({ options, close }: { options: InputOptions; close: (v: string | null) => void }) {
  const [value, setValue] = useState(options.value ?? "");
  const [touched, setTouched] = useState(false);
  const ref = useRef<HTMLInputElement & HTMLTextAreaElement>(null);
  const problem = options.validate?.(value) ?? null;
  const empty = !options.allowEmpty && value.trim() === "";

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.focus();
    if (options.selection) el.setSelectionRange(options.selection[0], options.selection[1]);
    else el.select();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const submit = () => {
    setTouched(true);
    if (!problem && !empty) close(value);
  };
  const onKeyDown = (e: ReactKeyboardEvent) => {
    if (e.key === "Enter" && (!options.multiline || e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      submit();
    }
  };
  const common = {
    ref,
    value,
    placeholder: options.placeholder,
    "aria-invalid": touched && !!problem,
    "aria-label": options.label ?? options.title,
    onChange: (e: { target: { value: string } }) => {
      setValue(e.target.value);
      setTouched(true);
    },
    onKeyDown,
  };
  return (
    <>
      <div className="oc-dialog-header">{options.title}</div>
      <div className="oc-dialog-body">
        {options.message && <p className="oc-selectable">{options.message}</p>}
        {options.label && <div className="oc-label" style={{ marginBottom: 4 }}>{options.label}</div>}
        {options.multiline ? (
          <textarea className="oc-textarea oc-mono" rows={8} {...common} />
        ) : (
          <input className="oc-input" type={options.password ? "password" : "text"} autoComplete="off" spellCheck={false} {...common} />
        )}
        {touched && problem && <div className="oc-error-text" style={{ marginTop: 4 }}>{problem}</div>}
        {options.multiline && <div className="oc-faint" style={{ marginTop: 4, fontSize: 11 }}>Ctrl+Enter to confirm</div>}
      </div>
      <div className="oc-dialog-footer">
        <button className="oc-btn" onClick={() => close(null)}>Cancel</button>
        <button className="oc-btn oc-btn-primary" disabled={!!problem || empty} onClick={submit}>
          {options.confirmLabel ?? "OK"}
        </button>
      </div>
    </>
  );
}

/** Menyorot karakter `text` pada indeks `matches`. */
export function Highlight({ text, matches }: { text: string; matches?: number[] }) {
  if (!matches || matches.length === 0) return <>{text}</>;
  const set = new Set(matches);
  const parts: ReactNode[] = [];
  let run = "";
  let marked = false;
  const flush = (key: number) => {
    if (!run) return;
    parts.push(marked ? <mark key={key}>{run}</mark> : <span key={key}>{run}</span>);
    run = "";
  };
  [...text].forEach((ch, i) => {
    const m = set.has(i);
    if (m !== marked) {
      flush(i);
      marked = m;
    }
    run += ch;
  });
  flush(text.length);
  return <>{parts}</>;
}

/** Pencocokan fuzzy sederhana: semua karakter kueri harus muncul berurutan. Skor lebih tinggi = lebih cocok. */
export function fuzzyMatch(query: string, text: string): { score: number; matches: number[] } | null {
  if (!query) return { score: 0, matches: [] };
  const q = query.toLowerCase();
  const t = text.toLowerCase();
  const matches: number[] = [];
  let score = 0;
  let ti = 0;
  let previous = -2;
  for (const ch of q) {
    if (ch === " ") continue;
    const found = t.indexOf(ch, ti);
    if (found < 0) return null;
    matches.push(found);
    const boundary = found === 0 || /[\s\-_./:]/.test(t[found - 1]) || (text[found] !== t[found] && text[found - 1] === t[found - 1]);
    score += found === previous + 1 ? 8 : boundary ? 6 : 1;
    score -= Math.min(found - ti, 6) * 0.2;
    previous = found;
    ti = found + 1;
  }
  return { score: score - text.length * 0.01, matches };
}

function QuickPick<T>({ options, close }: { options: PickOptions<T>; close: (v: T | null) => void }) {
  const [query, setQuery] = useState(options.initialQuery ?? "");
  const [asyncItems, setAsyncItems] = useState<PickItem<T>[] | null>(null);
  const [index, setIndex] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const source = options.items;

  useEffect(() => inputRef.current?.focus(), []);

  useEffect(() => {
    if (typeof source !== "function") return;
    let cancelled = false;
    void Promise.resolve(source(query)).then((items) => {
      if (!cancelled) {
        setAsyncItems(items);
        setIndex(0);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [query, source]);

  const items = useMemo(() => {
    if (typeof source === "function") return asyncItems ?? [];
    if (!query.trim()) return source;
    const scored: { item: PickItem<T>; score: number }[] = [];
    for (const item of source) {
      const byLabel = fuzzyMatch(query, item.label);
      if (byLabel) scored.push({ item: { ...item, matches: byLabel.matches }, score: byLabel.score });
      else if (item.description && fuzzyMatch(query, item.description)) scored.push({ item: { ...item, matches: [] }, score: -5 });
    }
    return scored.sort((x, y) => y.score - x.score).map((x) => x.item);
  }, [source, asyncItems, query]);

  useEffect(() => {
    listRef.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
  }, [index, items]);

  const onKeyDown = (e: ReactKeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setIndex((i) => Math.min(i + 1, Math.max(items.length - 1, 0)));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setIndex((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const item = items[Math.min(index, items.length - 1)];
      if (item) close(item.value);
    }
  };

  const visible = items.slice(0, 200);
  return (
    <div className="oc-palette" role="dialog" aria-label={options.placeholder ?? "Quick pick"}>
      <input
        ref={inputRef}
        className="oc-input oc-palette-input"
        placeholder={options.placeholder}
        value={query}
        spellCheck={false}
        onChange={(e) => {
          setQuery(e.target.value);
          setIndex(0);
        }}
        onKeyDown={onKeyDown}
      />
      <div className="oc-palette-list" ref={listRef} role="listbox">
        {visible.length === 0 && <div className="oc-palette-item oc-faint">{options.emptyText ?? "No results"}</div>}
        {visible.map((item, i) => (
          <div
            key={i}
            role="option"
            className="oc-palette-item"
            aria-selected={i === Math.min(index, visible.length - 1)}
            onMouseMove={() => setIndex(i)}
            onClick={() => close(item.value)}
          >
            <span className="oc-ellipsis">
              <Highlight text={item.label} matches={item.matches} />
            </span>
            {item.description && <span className="oc-faint oc-ellipsis">{item.description}</span>}
            {item.hint && <span className="oc-kbd">{item.hint}</span>}
          </div>
        ))}
      </div>
    </div>
  );
}

function Menu({ state }: { state: MenuState }) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ left: state.x, top: state.y });

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    setPos({
      left: Math.max(4, Math.min(state.x, window.innerWidth - rect.width - 4)),
      top: Math.max(4, Math.min(state.y, window.innerHeight - rect.height - 4)),
    });
    el.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
  }, [state]);

  useEffect(() => {
    const close = (e: Event) => {
      if (e.type === "mousedown" && ref.current?.contains(e.target as Node)) return;
      hideMenu();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        hideMenu();
        return;
      }
      if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
      e.preventDefault();
      const buttons = [...(ref.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? [])];
      const current = buttons.indexOf(document.activeElement as HTMLButtonElement);
      const next = (current + (e.key === "ArrowDown" ? 1 : -1) + buttons.length) % buttons.length;
      buttons[next]?.focus();
    };
    window.addEventListener("mousedown", close, true);
    window.addEventListener("blur", close);
    window.addEventListener("resize", close);
    window.addEventListener("wheel", close, true);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("mousedown", close, true);
      window.removeEventListener("blur", close);
      window.removeEventListener("resize", close);
      window.removeEventListener("wheel", close, true);
      window.removeEventListener("keydown", onKey, true);
    };
  }, []);

  return (
    <div ref={ref} className="oc-menu" role="menu" style={pos} onContextMenu={(e) => e.preventDefault()}>
      {state.items.map((item, i) =>
        item.separator ? (
          <div key={i} className="oc-menu-sep" role="separator" />
        ) : (
          <button
            key={i}
            role="menuitem"
            className="oc-menu-item"
            disabled={item.disabled}
            data-danger={item.danger || undefined}
            onClick={() => {
              hideMenu();
              void Promise.resolve(item.run?.()).catch((e) => showError(e));
            }}
          >
            <span className="oc-ellipsis">{item.label}</span>
            {item.hint && <span className="oc-faint" style={{ marginLeft: "auto", paddingLeft: 16 }}>{item.hint}</span>}
          </button>
        ),
      )}
    </div>
  );
}

/** Merender semua lapisan atas. Dipasang sekali di akar aplikasi. */
export function OverlayHost() {
  const { dialogs, toasts, menu } = useOverlay();

  useEffect(() => {
    if (dialogs.length === 0) return;
    const top = dialogs[dialogs.length - 1];
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && top.dismissible) {
        e.preventDefault();
        e.stopPropagation();
        closeDialog(top.id, null);
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [dialogs]);

  return (
    <>
      {dialogs.map((d) => (
        <div
          key={d.id}
          className="oc-overlay"
          onMouseDown={(e) => {
            if (e.target === e.currentTarget && d.dismissible) closeDialog(d.id, null);
          }}
        >
          {d.bare ? (
            d.render((value) => closeDialog(d.id, value))
          ) : (
            <div className={`oc-dialog ${d.wide ? "oc-dialog-wide" : ""}`} role="dialog" aria-modal="true">
              {d.render((value) => closeDialog(d.id, value))}
            </div>
          )}
        </div>
      ))}
      {menu && <Menu state={menu} />}
      <div className="oc-toasts" aria-live="polite">
        {toasts.map((t) => (
          <div key={t.id} className="oc-toast" data-kind={t.kind} role={t.kind === "error" ? "alert" : "status"}>
            <div className="oc-toast-message">
              {t.message}
              {t.actions.length > 0 && (
                <div className="oc-toast-actions">
                  {t.actions.map((a) => (
                    <button
                      key={a.label}
                      className="oc-btn oc-btn-sm"
                      onClick={() => {
                        dismissToast(t.id);
                        void Promise.resolve(a.run()).catch((e) => showError(e));
                      }}
                    >
                      {a.label}
                    </button>
                  ))}
                </div>
              )}
            </div>
            <button className="oc-iconbtn" aria-label="Dismiss" onClick={() => dismissToast(t.id)}>
              <X size={14} />
            </button>
          </div>
        ))}
      </div>
    </>
  );
}

/** Ada dialog terbuka (keybinding editor sebaiknya tidak aktif). */
export function hasOpenDialog(): boolean {
  return useOverlay.getState().dialogs.length > 0;
}

/** Hanya untuk pengujian. */
export function resetOverlayForTest(): void {
  useOverlay.setState({ dialogs: [], toasts: [], menu: null });
}
