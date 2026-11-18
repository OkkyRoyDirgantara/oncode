// Kendali aliran keluaran terminal: bila xterm tertinggal memproses, pembacaan PTY di backend dijeda.

/** Jeda saat antrean melebihi angka ini (dalam karakter). */
export const HIGH_WATERMARK = 500_000;
/** Lanjutkan saat antrean turun di bawah angka ini. */
export const LOW_WATERMARK = 100_000;

/**
 * Menghitung karakter yang sudah diserahkan ke xterm tetapi belum selesai diproses.
 * `setPaused` hanya dipanggil saat keadaan berubah, tidak pada setiap potongan.
 */
export class FlowControl {
  private pending = 0;
  private paused = false;

  constructor(
    private readonly setPaused: (paused: boolean) => void,
    private readonly high = HIGH_WATERMARK,
    private readonly low = LOW_WATERMARK,
  ) {
    if (low > high) throw new Error("low watermark harus tidak lebih besar dari high watermark");
  }

  /** Sebuah potongan diserahkan ke xterm. */
  written(length: number): void {
    this.pending += length;
    if (!this.paused && this.pending > this.high) {
      this.paused = true;
      this.setPaused(true);
    }
  }

  /** xterm selesai memproses sebuah potongan (callback `write`). */
  processed(length: number): void {
    this.pending = Math.max(0, this.pending - length);
    if (this.paused && this.pending < this.low) {
      this.paused = false;
      this.setPaused(false);
    }
  }

  get isPaused(): boolean {
    return this.paused;
  }

  get pendingChars(): number {
    return this.pending;
  }
}
