// Batas antara logika sesi terminal dan xterm.js. Store hanya mengenal antarmuka ini,
// sehingga pengujian bisa memakai adapter tiruan (xterm butuh kanvas dan tata letak sungguhan).

export interface TerminalSize {
  cols: number;
  rows: number;
}

export interface AdapterEvents {
  /** Masukan pengguna (ketikan atau tempelan) yang harus dikirim ke PTY. */
  onInput(data: string): void;
  /** Ukuran berubah setelah penyesuaian terhadap wadahnya; hanya terjadi saat terminal terlihat. */
  onResize(size: TerminalSize): void;
  /** Pengguna meminta bilah cari (Mod+F). */
  onFind(): void;
}

export interface TerminalAdapter {
  /**
   * Memasang terminal ke `stage` (elemennya tetap terpasang selama sesi hidup) dan
   * mengembalikan ukuran awal. Boleh dipanggil lagi dengan stage lain untuk memindahkannya.
   */
  mount(stage: HTMLElement): Promise<TerminalSize>;
  /** Menulis keluaran PTY; `done` dipanggil setelah potongan ini selesai diproses. */
  write(data: string, done?: () => void): void;
  /** Tampilkan atau sembunyikan (CSS). Penyesuaian ukuran hanya dilakukan saat terlihat. */
  setVisible(visible: boolean): void;
  focus(): void;
  clear(): void;
  /** Mencari teks di buffer; mengembalikan true bila ada kecocokan. */
  find(query: string, forward: boolean): boolean;
  clearFind(): void;
  dispose(): void;
}

export type AdapterFactory = (events: AdapterEvents) => TerminalAdapter | Promise<TerminalAdapter>;

export const DEFAULT_SIZE: TerminalSize = { cols: 80, rows: 24 };
