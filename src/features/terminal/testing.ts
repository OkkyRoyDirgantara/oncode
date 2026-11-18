// Adapter tiruan untuk pengujian: mencatat apa yang ditulis dan membiarkan pengujian
// memicu masukan/perubahan ukuran seolah-olah datang dari xterm.
import type { AdapterEvents, AdapterFactory, TerminalAdapter, TerminalSize } from "./adapter";

export interface FakeAdapter extends TerminalAdapter {
  events: AdapterEvents;
  written: string[];
  /** Callback `write` yang belum dipanggil (hanya terisi bila `autoProcess` false). */
  pendingDone: (() => void)[];
  autoProcess: boolean;
  size: TerminalSize;
  stage: HTMLElement | null;
  visible: boolean;
  focusCount: number;
  clearCount: number;
  disposed: boolean;
}

export function fakeAdapterFactory(options: { size?: TerminalSize; autoProcess?: boolean } = {}): { factory: AdapterFactory; adapters: FakeAdapter[] } {
  const adapters: FakeAdapter[] = [];
  const factory: AdapterFactory = (events) => {
    const adapter: FakeAdapter = {
      events,
      written: [],
      pendingDone: [],
      autoProcess: options.autoProcess ?? true,
      size: options.size ?? { cols: 100, rows: 30 },
      stage: null,
      visible: false,
      focusCount: 0,
      clearCount: 0,
      disposed: false,
      mount(stage) {
        adapter.stage = stage;
        return Promise.resolve(adapter.size);
      },
      write(data, done) {
        adapter.written.push(data);
        if (!done) return;
        if (adapter.autoProcess) done();
        else adapter.pendingDone.push(done);
      },
      setVisible(visible) {
        adapter.visible = visible;
      },
      focus() {
        adapter.focusCount += 1;
      },
      clear() {
        adapter.clearCount += 1;
      },
      find: (query) => adapter.written.join("").includes(query),
      clearFind() {},
      dispose() {
        adapter.disposed = true;
      },
    };
    adapters.push(adapter);
    return adapter;
  };
  return { factory, adapters };
}
