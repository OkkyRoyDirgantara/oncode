//! Model diff baris dan penerjemahannya menjadi operasi sambung-potong (`SpliceOp`) untuk
//! staging per hunk dan per baris.
//!
//! Ada dua cara menghitung baris yang harus dibedakan:
//!
//! - **Baris nyata**: potongan isi yang diakhiri `\n` (baris terakhir boleh tanpa `\n`).
//!   Inilah yang ditampilkan tampilan diff dan yang dibandingkan di sini, sehingga baris
//!   terakhir tanpa newline berbeda dari baris yang sama dengan newline (seperti di Git).
//! - **Baris model**: hasil `isi.split('\n')`, yang dipakai backend (`splice_forward`).
//!   `"a\nb\n"` adalah `["a", "b", ""]`; elemen kosong di akhir disebut *baris maya* dan
//!   menandakan bahwa isi diakhiri newline (atau kosong).
//!
//! Indeks baris nyata sama dengan indeks baris modelnya; yang berbeda hanya baris maya di
//! akhir. Semua operasi yang menyentuh akhir berkas memperhitungkan baris maya itu.

use std::ops::Range;
use std::time::{Duration, Instant};

use oncode_git::SpliceOp;
use similar::{Algorithm, DiffTag};

/// Satu kelompok perubahan yang bersambung: `A[a_from..a_to)` menjadi `B[b_from..b_to)`,
/// dalam indeks baris nyata mulai dari 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chunk {
    pub a_from: u32,
    pub a_to: u32,
    pub b_from: u32,
    pub b_to: u32,
}

impl Chunk {
    pub fn a_len(&self) -> u32 {
        self.a_to - self.a_from
    }

    pub fn b_len(&self) -> u32 {
        self.b_to - self.b_from
    }
}

/// Arah penerapan operasi di backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Hasil = A dengan bagian terpilih diganti dari B (stage).
    Forward,
    /// Hasil = B dengan bagian terpilih dikembalikan dari A (unstage, discard).
    Reverse,
}

/// Baris yang dipilih pengguna di tampilan diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineSelection {
    /// Baris sisi B `[from, to)`. Pilihan yang mencakup seluruh sisi B sebuah perubahan
    /// memakai perubahan itu seutuhnya; penghapusan murni ikut bila pilihan menyentuh posisinya.
    Modified(Range<u32>),
    /// Baris sisi A `[from, to)`: hanya baris yang dihapus.
    Original(Range<u32>),
    /// Rentang baris tampilan unified (inklusif), lihat [`LineDiff::row_of`].
    Rows(u64, u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Original,
    Modified,
}

/// Hasil perbandingan dua isi berkas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineDiff {
    /// Jumlah baris nyata tiap sisi.
    pub a_lines: u32,
    pub b_lines: u32,
    /// Sisi ini punya baris maya (isi kosong atau diakhiri newline).
    pub a_virtual: bool,
    pub b_virtual: bool,
    pub chunks: Vec<Chunk>,
}

/// Baris nyata sebuah isi, masing-masing beserta akhir barisnya.
pub fn real_lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

fn op(a_from: u32, a_to: u32, b_from: u32, b_to: u32) -> SpliceOp {
    SpliceOp { a_from, a_to, b_from, b_to }
}

impl LineDiff {
    /// Membandingkan dua isi baris demi baris. Untuk berkas sangat besar perbandingan
    /// dibatasi waktu; hasilnya tetap benar, hanya mungkin kurang ringkas.
    pub fn compute(a: &str, b: &str) -> Self {
        let (a_tokens, b_tokens) = (real_lines(a), real_lines(b));
        let deadline = Instant::now() + Duration::from_secs(3);
        let ops = similar::capture_diff_slices_deadline(Algorithm::Myers, &a_tokens, &b_tokens, Some(deadline));
        let mut chunks: Vec<Chunk> = Vec::new();
        for item in ops {
            let (tag, old, new) = item.as_tag_tuple();
            if tag == DiffTag::Equal {
                continue;
            }
            let next = Chunk { a_from: old.start as u32, a_to: old.end as u32, b_from: new.start as u32, b_to: new.end as u32 };
            match chunks.last_mut() {
                // Hapus lalu sisip yang bersebelahan adalah satu perubahan.
                Some(last) if last.a_to == next.a_from && last.b_to == next.b_from => {
                    last.a_to = next.a_to;
                    last.b_to = next.b_to;
                }
                _ => chunks.push(next),
            }
        }
        Self {
            a_lines: a_tokens.len() as u32,
            b_lines: b_tokens.len() as u32,
            a_virtual: a.is_empty() || a.ends_with('\n'),
            b_virtual: b.is_empty() || b.ends_with('\n'),
            chunks,
        }
    }

    /// Jumlah baris model tiap sisi.
    fn model_lens(&self) -> (u32, u32) {
        (self.a_lines + u32::from(self.a_virtual), self.b_lines + u32::from(self.b_virtual))
    }

    /// Perubahan ini mencapai akhir kedua sisi (keduanya selalu bersamaan).
    fn at_end(&self, chunk: &Chunk) -> bool {
        chunk.a_to == self.a_lines && chunk.b_to == self.b_lines
    }

    /// Operasi untuk satu perubahan seutuhnya. Berlaku untuk kedua arah.
    pub fn chunk_op(&self, chunk: &Chunk) -> SpliceOp {
        if self.at_end(chunk) {
            // Baris maya ikut, agar ada/tidaknya newline di akhir berkas mengikuti sisi sumber.
            let (a_len, b_len) = self.model_lens();
            op(chunk.a_from, a_len, chunk.b_from, b_len)
        } else {
            op(chunk.a_from, chunk.a_to, chunk.b_from, chunk.b_to)
        }
    }

    /// Operasi untuk semua perubahan.
    pub fn all_ops(&self) -> Vec<SpliceOp> {
        self.chunks.iter().map(|chunk| self.chunk_op(chunk)).collect()
    }

    /// Posisi sebuah baris nyata pada urutan tampilan unified (baris yang dihapus mendahului
    /// baris yang ditambahkan dalam satu perubahan; baris tak berubah dihitung sekali).
    pub fn row_of(&self, side: Side, line: u32) -> u64 {
        let mut row = u64::from(line);
        for chunk in &self.chunks {
            match side {
                Side::Original if chunk.a_to <= line => row += u64::from(chunk.b_len()),
                Side::Modified if chunk.b_from <= line => row += u64::from(chunk.a_len()),
                _ => {}
            }
        }
        row
    }

    /// Operasi untuk baris terpilih saja.
    ///
    /// Mengikuti urutan patch Git. Maju (stage): baris lama yang tidak dipilih dipertahankan
    /// dan baris baru yang dipilih disisipkan setelahnya. Mundur (unstage/discard): baris
    /// baru yang dipilih dibuang dan baris lama yang dipilih dikembalikan sebelum sisa baris baru.
    pub fn selection_ops(&self, selection: &LineSelection, direction: Direction) -> Vec<SpliceOp> {
        let mut ops = Vec::new();
        // Jumlah baris sisi lawan pada perubahan-perubahan sebelum yang sedang diperiksa.
        let (mut added_before, mut deleted_through) = (0u64, 0u64);
        for chunk in &self.chunks {
            deleted_through += u64::from(chunk.a_len());
            let clip = |range: &Range<u32>, from: u32, to: u32| (range.start.max(from), range.end.min(to).max(range.start.max(from)));
            // (baris dihapus terpilih, baris ditambah terpilih, pakai seluruh perubahan)
            let (deleted, added, whole) = match selection {
                LineSelection::Modified(range) => {
                    if chunk.b_len() == 0 {
                        let touched = range.start <= chunk.b_from && chunk.b_from <= range.end;
                        ((chunk.a_from, chunk.a_from), (chunk.b_from, chunk.b_from), touched)
                    } else {
                        let added = clip(range, chunk.b_from, chunk.b_to);
                        ((chunk.a_from, chunk.a_from), added, added == (chunk.b_from, chunk.b_to))
                    }
                }
                LineSelection::Original(range) => {
                    let deleted = clip(range, chunk.a_from, chunk.a_to);
                    let whole = chunk.b_len() == 0 && chunk.a_len() > 0 && deleted == (chunk.a_from, chunk.a_to);
                    (deleted, (chunk.b_from, chunk.b_from), whole)
                }
                LineSelection::Rows(low, high) => {
                    // Baris A ke-i perubahan ini ada di baris tampilan `i + added_before`;
                    // baris B ke-j ada di `j + deleted_through`.
                    let side = |from: u32, to: u32, offset: u64| {
                        let start = (*low).saturating_sub(offset).max(u64::from(from)).min(u64::from(to)) as u32;
                        let end = (high + 1).saturating_sub(offset).min(u64::from(to)).max(u64::from(start)) as u32;
                        (start, end)
                    };
                    let deleted = side(chunk.a_from, chunk.a_to, added_before);
                    let added = side(chunk.b_from, chunk.b_to, deleted_through);
                    (deleted, added, deleted == (chunk.a_from, chunk.a_to) && added == (chunk.b_from, chunk.b_to))
                }
            };
            added_before += u64::from(chunk.b_len());
            if whole {
                ops.push(self.chunk_op(chunk));
            } else if deleted.0 < deleted.1 || added.0 < added.1 {
                self.partial_ops(chunk, deleted, added, direction, &mut ops);
            }
        }
        ops
    }

    /// Operasi untuk sebagian baris sebuah perubahan.
    fn partial_ops(&self, chunk: &Chunk, deleted: (u32, u32), added: (u32, u32), direction: Direction, ops: &mut Vec<SpliceOp>) {
        let (has_deleted, has_added) = (deleted.0 < deleted.1, added.0 < added.1);
        if has_deleted {
            ops.push(op(deleted.0, deleted.1, chunk.b_from, chunk.b_from));
        }
        if has_added {
            ops.push(op(chunk.a_to, chunk.a_to, added.0, added.1));
        }
        if !self.at_end(chunk) {
            return;
        }
        // Perubahan di akhir berkas: tentukan apakah hasilnya harus diakhiri newline. Tiap baris
        // membawa akhir barisnya sendiri; yang menentukan adalah baris terakhir hasil.
        let (a_len, b_len) = self.model_lens();
        let a_last_terminated = deleted.1 < self.a_lines || self.a_virtual;
        let b_last_terminated = added.1 < self.b_lines || self.b_virtual;
        match direction {
            Direction::Forward => {
                // Hasil: baris A yang dipertahankan, lalu baris B terpilih.
                let want = if has_added {
                    b_last_terminated
                } else if deleted.1 < self.a_lines {
                    self.a_virtual
                } else {
                    true
                };
                if want && !self.a_virtual && self.b_virtual {
                    ops.push(op(a_len, a_len, self.b_lines, b_len));
                } else if !want && self.a_virtual {
                    ops.push(op(self.a_lines, a_len, b_len, b_len));
                }
            }
            Direction::Reverse => {
                // Hasil: baris A yang dikembalikan, lalu baris B yang tidak dipilih.
                let keeps_last_b = if has_added { added.1 < self.b_lines } else { chunk.b_len() > 0 };
                let want = if keeps_last_b {
                    self.b_virtual
                } else if has_added && added.0 > chunk.b_from {
                    true
                } else if has_deleted {
                    a_last_terminated
                } else {
                    true
                };
                if want && !self.b_virtual && self.a_virtual {
                    ops.push(op(self.a_lines, a_len, b_len, b_len));
                } else if !want && self.b_virtual {
                    ops.push(op(a_len, a_len, self.b_lines, b_len));
                }
            }
        }
    }

    /// Baris terakhir sebuah perubahan pada tampilan: sisi B bila ada baris baru, selain itu
    /// sisi A. Nomor baris mulai dari 1.
    pub fn chunk_anchor(&self, chunk: &Chunk) -> (Side, u32) {
        if chunk.b_len() > 0 {
            (Side::Modified, chunk.b_to)
        } else {
            (Side::Original, chunk.a_to)
        }
    }

    /// Baris pertama (mulai dari 1) sisi B yang berubah, untuk "Open File".
    pub fn first_changed_line(&self) -> Option<u32> {
        self.chunks.first().map(|chunk| (chunk.b_from + 1).min(self.b_lines.max(1)))
    }

    /// Patch unified untuk ditampilkan. `context`: jumlah baris tak berubah di sekitar tiap
    /// perubahan; `None` menyertakan seluruh berkas dalam satu hunk.
    pub fn unified_patch(&self, a: &str, b: &str, name: &str, context: Option<u32>) -> String {
        let (a_tokens, b_tokens) = (real_lines(a), real_lines(b));
        let mut out = String::with_capacity(a.len() + b.len() + 128);
        out.push_str(&format!("--- {}\n+++ {}\n", if self.a_lines == 0 { "/dev/null".to_string() } else { format!("a/{name}") }, if self.b_lines == 0 { "/dev/null".to_string() } else { format!("b/{name}") }));
        if self.chunks.is_empty() {
            return out;
        }
        // Kelompokkan perubahan menjadi hunk: (indeks perubahan pertama, terakhir, awal A, akhir A).
        let mut hunks: Vec<(usize, usize, u32, u32)> = Vec::new();
        match context {
            None => hunks.push((0, self.chunks.len() - 1, 0, self.a_lines)),
            Some(context) => {
                for (index, chunk) in self.chunks.iter().enumerate() {
                    let start = chunk.a_from.saturating_sub(context);
                    let end = (chunk.a_to + context).min(self.a_lines);
                    match hunks.last_mut() {
                        Some(last) if start <= last.3 => {
                            last.1 = index;
                            last.3 = end;
                        }
                        _ => hunks.push((index, index, start, end)),
                    }
                }
            }
        }
        let push_line = |out: &mut String, sign: char, token: &str| {
            out.push(sign);
            match token.strip_suffix('\n') {
                Some(content) => {
                    out.push_str(content);
                    out.push('\n');
                }
                None => {
                    out.push_str(token);
                    out.push_str("\n\\ No newline at end of file\n");
                }
            }
        };
        for (first, last, a_start, a_end) in hunks {
            let chunks = &self.chunks[first..=last];
            // Posisi B yang bersesuaian dengan awal dan akhir hunk.
            let b_start = chunks[0].b_from - (chunks[0].a_from - a_start);
            let b_end = chunks[chunks.len() - 1].b_to + (a_end - chunks[chunks.len() - 1].a_to);
            let (a_count, b_count) = (a_end - a_start, b_end - b_start);
            // Menurut format unified: hunk tanpa baris di satu sisi menunjuk baris sebelumnya.
            let start = |from: u32, count: u32| if count == 0 { from } else { from + 1 };
            out.push_str(&format!("@@ -{},{} +{},{} @@\n", start(a_start, a_count), a_count, start(b_start, b_count), b_count));
            let mut a_cursor = a_start;
            for chunk in chunks {
                for token in &a_tokens[a_cursor as usize..chunk.a_from as usize] {
                    push_line(&mut out, ' ', token);
                }
                for token in &a_tokens[chunk.a_from as usize..chunk.a_to as usize] {
                    push_line(&mut out, '-', token);
                }
                for token in &b_tokens[chunk.b_from as usize..chunk.b_to as usize] {
                    push_line(&mut out, '+', token);
                }
                a_cursor = chunk.a_to;
            }
            for token in &a_tokens[a_cursor as usize..a_end as usize] {
                push_line(&mut out, ' ', token);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oncode_git::content::{splice_forward, splice_reverse};

    fn forward(a: &str, b: &str, ops: &[SpliceOp]) -> String {
        String::from_utf8(splice_forward(a.as_bytes(), b.as_bytes(), ops).expect("operasi sah")).unwrap()
    }

    fn reverse(a: &str, b: &str, ops: &[SpliceOp]) -> String {
        String::from_utf8(splice_reverse(a.as_bytes(), b.as_bytes(), ops).expect("operasi sah")).unwrap()
    }

    const CASES: &[(&str, &str)] = &[
        ("1\n2\n3\n4\n5\n", "1\nDUA\n3\n4\nLIMA\nENAM\n"),
        ("a\nb\n", "a\nb\nc\n"),
        ("a\nb", "a\nb\nc"),
        ("a\nb", "a\nb\nc\n"),
        ("a\nb\n", "a\nb"),
        ("a\nb\nc\n", "a\nc\n"),
        ("a\nb\nc\n", "b\nc\n"),
        ("a\nb\nc\n", "a\nb\n"),
        ("", "x\ny\n"),
        ("x\ny\n", ""),
        ("", "x\ny"),
        ("x\ny", ""),
        ("same\n", "same\n"),
        ("a\n\n\nb\n", "a\nb\n"),
        ("a\r\nb\r\n", "a\r\nB\r\nc\r\n"),
        ("fn a() {\n}\n\nfn b() {\n}\n", "fn a() {\n}\n\nfn x() {\n}\n\nfn b() {\n}\n"),
    ];

    #[test]
    fn applying_every_chunk_yields_the_other_side() {
        for (a, b) in CASES {
            let diff = LineDiff::compute(a, b);
            assert_eq!(&forward(a, b, &diff.all_ops()), b, "maju {a:?} → {b:?}");
            assert_eq!(&reverse(a, b, &diff.all_ops()), a, "mundur {a:?} → {b:?}");
        }
        assert!(LineDiff::compute("same\n", "same\n").chunks.is_empty());
    }

    #[test]
    fn applying_one_chunk_changes_only_that_part() {
        let a = "1\n2\n3\n4\n5\n6\n7\n8\n9\n";
        let b = "1\nDUA\n3\n4\n5\n6\n7\nDELAPAN\n9\n";
        let diff = LineDiff::compute(a, b);
        assert_eq!(diff.chunks.len(), 2);
        let (first, second) = (diff.chunk_op(&diff.chunks[0]), diff.chunk_op(&diff.chunks[1]));
        assert_eq!(forward(a, b, &[first]), "1\nDUA\n3\n4\n5\n6\n7\n8\n9\n");
        assert_eq!(forward(a, b, &[second]), "1\n2\n3\n4\n5\n6\n7\nDELAPAN\n9\n");
        // Mundur: hanya perubahan itu yang dikembalikan.
        assert_eq!(reverse(a, b, &[first]), "1\n2\n3\n4\n5\n6\n7\nDELAPAN\n9\n");
    }

    #[test]
    fn trailing_newline_changes_are_part_of_the_last_chunk() {
        // Baris terakhir tanpa newline berbeda dari baris yang sama dengan newline.
        let diff = LineDiff::compute("a\nb", "a\nb\n");
        assert_eq!(diff.chunks, vec![Chunk { a_from: 1, a_to: 2, b_from: 1, b_to: 2 }]);
        assert_eq!(diff.chunk_op(&diff.chunks[0]), op(1, 2, 1, 3));
        // Berkas baru: sisi A hanya berisi baris maya.
        let diff = LineDiff::compute("", "x\ny\n");
        assert_eq!((diff.a_lines, diff.a_virtual, diff.b_lines, diff.b_virtual), (0, true, 2, true));
        assert_eq!(diff.all_ops(), vec![op(0, 1, 0, 3)]);
        // Perubahan di tengah tidak menyentuh baris maya.
        let diff = LineDiff::compute("a\nb\n", "a\nX\nb\n");
        assert_eq!(diff.all_ops(), vec![op(1, 1, 1, 2)]);
    }

    mod selection {
        use super::*;

        const A: &str = "1\n2\n3\n";
        const B: &str = "1\nA\nB\nC\n3\n";

        fn modified(a: &str, b: &str, from: u32, to: u32) -> Vec<SpliceOp> {
            LineDiff::compute(a, b).selection_ops(&LineSelection::Modified(from..to), Direction::Forward)
        }

        #[test]
        fn partial_selection_inserts_the_picked_lines_after_the_old_ones() {
            // Hanya baris "B" (indeks 2).
            let picked = modified(A, B, 2, 3);
            assert_eq!(picked, vec![op(2, 2, 2, 3)]);
            assert_eq!(forward(A, B, &picked), "1\n2\nB\n3\n");
        }

        #[test]
        fn selection_covering_the_whole_chunk_uses_the_chunk() {
            assert_eq!(forward(A, B, &modified(A, B, 0, 5)), B);
            assert_eq!(forward(A, B, &modified(A, B, 1, 4)), B);
        }

        #[test]
        fn selection_outside_any_chunk_yields_nothing() {
            assert!(modified(A, B, 0, 1).is_empty());
        }

        #[test]
        fn pure_deletion_is_picked_when_the_selection_touches_it() {
            let (a, b) = ("1\n2\n3\n", "1\n3\n");
            assert_eq!(forward(a, b, &modified(a, b, 1, 1)), b);
            assert!(modified(a, b, 2, 2).is_empty());
        }

        #[test]
        fn deleted_lines_can_be_picked_on_the_original_side() {
            let (a, b) = ("1\n2\n3\n4\n", "1\nX\n4\n");
            let diff = LineDiff::compute(a, b);
            // Hanya penghapusan baris "3"; baris "2" tetap dan "X" tidak ikut.
            let ops = diff.selection_ops(&LineSelection::Original(2..3), Direction::Forward);
            assert_eq!(forward(a, b, &ops), "1\n2\n4\n");
            // Mundur (unstage): baris "3" kembali, sebelum baris baru yang tersisa.
            assert_eq!(reverse(a, b, &ops), "1\n3\nX\n4\n");
            // Penghapusan murni yang seluruhnya dipilih memakai perubahan utuh.
            let (a, b) = ("1\n2\n3\n", "1\n3\n");
            let diff = LineDiff::compute(a, b);
            assert_eq!(diff.selection_ops(&LineSelection::Original(1..2), Direction::Forward), diff.all_ops());
            assert!(diff.selection_ops(&LineSelection::Original(0..1), Direction::Forward).is_empty());
        }

        #[test]
        fn unified_rows_select_across_both_sides() {
            let (a, b) = ("1\n2\n3\n4\n", "1\nX\nY\n4\n");
            let diff = LineDiff::compute(a, b);
            // Urutan tampilan: 1, -2, -3, +X, +Y, 4.
            assert_eq!(diff.row_of(Side::Original, 0), 0);
            assert_eq!(diff.row_of(Side::Original, 2), 2);
            assert_eq!(diff.row_of(Side::Modified, 1), 3);
            assert_eq!(diff.row_of(Side::Modified, 3), 5);
            assert_eq!(diff.row_of(Side::Original, 3), 5);
            // "-3" sampai "+X".
            let ops = diff.selection_ops(&LineSelection::Rows(2, 3), Direction::Forward);
            assert_eq!(forward(a, b, &ops), "1\n2\nX\n4\n");
            // Seluruh perubahan.
            assert_eq!(diff.selection_ops(&LineSelection::Rows(1, 4), Direction::Forward), diff.all_ops());
            // Hanya baris tak berubah.
            assert!(diff.selection_ops(&LineSelection::Rows(5, 5), Direction::Forward).is_empty());
        }

        #[test]
        fn reverse_removes_only_the_picked_added_lines() {
            let diff = LineDiff::compute(A, B);
            let ops = diff.selection_ops(&LineSelection::Modified(2..3), Direction::Reverse);
            assert_eq!(reverse(A, B, &ops), "1\nA\nC\n3\n");
        }

        #[test]
        fn partial_selection_in_a_new_file() {
            // Berkas baru (sisi A tidak ada): sebagian baris tetap diakhiri newline.
            let (a, b) = ("", "x\ny\nz\n");
            let diff = LineDiff::compute(a, b);
            let ops = diff.selection_ops(&LineSelection::Modified(1..2), Direction::Forward);
            assert_eq!(forward(a, b, &ops), "y\n");
            // Baris terakhir B tanpa newline: memilihnya menghasilkan isi tanpa newline juga,
            // memilih baris lain tetap diakhiri newline.
            let (a, b) = ("", "x\ny");
            let diff = LineDiff::compute(a, b);
            assert_eq!(forward(a, b, &diff.selection_ops(&LineSelection::Modified(1..2), Direction::Forward)), "y");
            assert_eq!(forward(a, b, &diff.selection_ops(&LineSelection::Modified(0..1), Direction::Forward)), "x\n");
            // Mundur: membuang "y" dari B menyisakan "x\n".
            assert_eq!(reverse(a, b, &diff.selection_ops(&LineSelection::Modified(1..2), Direction::Reverse)), "x\n");
        }

        #[test]
        fn partial_selection_in_a_deleted_file() {
            let (a, b) = ("x\ny\nz\n", "");
            let diff = LineDiff::compute(a, b);
            let ops = diff.selection_ops(&LineSelection::Original(1..2), Direction::Forward);
            assert_eq!(forward(a, b, &ops), "x\nz\n");
            // Mundur (unstage penghapusan satu baris): hanya baris itu yang kembali.
            assert_eq!(reverse(a, b, &ops), "y\n");
            // Semua baris terpilih: perubahan utuh.
            assert_eq!(diff.selection_ops(&LineSelection::Original(0..3), Direction::Forward), diff.all_ops());
        }

        #[test]
        fn partial_selection_at_the_end_without_trailing_newline() {
            // A tanpa newline di akhir; baris baru disisipkan setelah baris lama.
            let (a, b) = ("a", "a\nb\nc\n");
            let diff = LineDiff::compute(a, b);
            assert_eq!(diff.chunks, vec![Chunk { a_from: 0, a_to: 1, b_from: 0, b_to: 3 }]);
            assert_eq!(forward(a, b, &diff.selection_ops(&LineSelection::Modified(1..2), Direction::Forward)), "a\nb\n");
            assert_eq!(forward(a, b, &diff.selection_ops(&LineSelection::Modified(2..3), Direction::Forward)), "a\nc\n");
            // Menghapus baris terakhir yang tanpa newline: baris sebelumnya tetap diakhiri newline.
            let (a, b) = ("a\nb\nc", "a\n");
            let diff = LineDiff::compute(a, b);
            assert_eq!(forward(a, b, &diff.selection_ops(&LineSelection::Original(2..3), Direction::Forward)), "a\nb\n");
            assert_eq!(forward(a, b, &diff.selection_ops(&LineSelection::Original(1..2), Direction::Forward)), "a\nc");
        }

        /// Hasil yang diharapkan, dihitung langsung dari baris nyata: tiap baris membawa akhir
        /// barisnya sendiri, dan baris yang bukan terakhir selalu diakhiri newline.
        fn expected(a: &str, b: &str, diff: &LineDiff, chunk_index: usize, deleted: Range<u32>, added: Range<u32>, direction: Direction) -> String {
            let (a_tokens, b_tokens) = (real_lines(a), real_lines(b));
            let chunk = diff.chunks[chunk_index];
            let slice = |tokens: &[&str], from: u32, to: u32| tokens[from as usize..to as usize].iter().map(|t| t.to_string()).collect::<Vec<_>>();
            let mut out: Vec<String> = Vec::new();
            match direction {
                Direction::Forward => {
                    out.extend(slice(&a_tokens, 0, deleted.start));
                    out.extend(slice(&a_tokens, deleted.end, chunk.a_to));
                    out.extend(slice(&b_tokens, added.start, added.end));
                    out.extend(slice(&a_tokens, chunk.a_to, diff.a_lines));
                }
                Direction::Reverse => {
                    out.extend(slice(&b_tokens, 0, chunk.b_from));
                    out.extend(slice(&a_tokens, deleted.start, deleted.end));
                    out.extend(slice(&b_tokens, chunk.b_from, added.start));
                    out.extend(slice(&b_tokens, added.end, diff.b_lines));
                }
            }
            let count = out.len();
            for (index, line) in out.iter_mut().enumerate() {
                if index + 1 < count && !line.ends_with('\n') {
                    line.push('\n');
                }
            }
            out.concat()
        }

        #[test]
        fn every_partial_selection_matches_the_line_model() {
            let cases: &[(&str, &str)] = &[
                ("1\n2\n3\n", "1\nA\nB\nC\n3\n"),
                ("1\n2\n3\n4\n", "1\nX\nY\n4\n"),
                ("", "x\ny\nz\n"),
                ("", "x\ny\nz"),
                ("x\ny\nz\n", ""),
                ("x\ny\nz", ""),
                ("a", "a\nb\nc\n"),
                ("a\nb\nc\n", "a"),
                ("a\nb\nc", "a\nX\nY\n"),
                ("a\nb\nc\n", "a\nX\nY"),
                ("a\nb\nc", "a\nX\nY"),
                ("a\nb\nc\n", "a\nX\nY\n"),
                ("k\na\nb", "k\nb\n"),
            ];
            for (a, b) in cases {
                let diff = LineDiff::compute(a, b);
                for (index, chunk) in diff.chunks.iter().enumerate() {
                    for d0 in chunk.a_from..=chunk.a_to {
                        for d1 in d0..=chunk.a_to {
                            for s0 in chunk.b_from..=chunk.b_to {
                                for s1 in s0..=chunk.b_to {
                                    if d0 == d1 && s0 == s1 {
                                        continue;
                                    }
                                    for direction in [Direction::Forward, Direction::Reverse] {
                                        let mut ops = Vec::new();
                                        if (d0, d1) == (chunk.a_from, chunk.a_to) && (s0, s1) == (chunk.b_from, chunk.b_to) {
                                            ops.push(diff.chunk_op(chunk));
                                        } else {
                                            diff.partial_ops(chunk, (d0, d1), (s0, s1), direction, &mut ops);
                                        }
                                        let actual = match direction {
                                            Direction::Forward => forward(a, b, &ops),
                                            Direction::Reverse => reverse(a, b, &ops),
                                        };
                                        let want = expected(a, b, &diff, index, d0..d1, s0..s1, direction);
                                        // Newline di akhir hanya bisa ditambahkan bila sisi sumbernya punya;
                                        // kalau tidak, hasilnya sama kecuali newline terakhir itu.
                                        let source_has_newline = match direction {
                                            Direction::Forward => diff.b_virtual,
                                            Direction::Reverse => diff.a_virtual,
                                        };
                                        if actual != want {
                                            assert!(
                                                !source_has_newline && want.strip_suffix('\n') == Some(actual.as_str()),
                                                "{a:?} → {b:?}, hapus {d0}..{d1}, tambah {s0}..{s1}, {direction:?}: {actual:?} ≠ {want:?}"
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    mod patch {
        use super::*;

        #[test]
        fn full_context_patch_is_a_single_hunk() {
            let (a, b) = ("1\n2\n3\n4\n5\n", "1\nDUA\n3\n4\nLIMA\nENAM\n");
            let patch = LineDiff::compute(a, b).unified_patch(a, b, "f.txt", None);
            assert_eq!(patch, "--- a/f.txt\n+++ b/f.txt\n@@ -1,5 +1,6 @@\n 1\n-2\n+DUA\n 3\n 4\n-5\n+LIMA\n+ENAM\n");
        }

        #[test]
        fn limited_context_splits_distant_changes_into_hunks() {
            let a: String = (1..=20).map(|n| format!("{n}\n")).collect();
            let b = a.replace("2\n3\n", "2\nX\n3\n").replace("18\n", "DELAPAN BELAS\n");
            let patch = LineDiff::compute(&a, &b).unified_patch(&a, &b, "f", Some(1));
            assert_eq!(patch, "--- a/f\n+++ b/f\n@@ -2,2 +2,3 @@\n 2\n+X\n 3\n@@ -17,3 +18,3 @@\n 17\n-18\n+DELAPAN BELAS\n 19\n");
        }

        #[test]
        fn marks_missing_trailing_newlines_and_missing_sides() {
            let patch = LineDiff::compute("a\nb", "a\nb\n").unified_patch("a\nb", "a\nb\n", "f", None);
            assert_eq!(patch, "--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+b\n");
            let patch = LineDiff::compute("", "x\ny").unified_patch("", "x\ny", "f", None);
            assert_eq!(patch, "--- /dev/null\n+++ b/f\n@@ -0,0 +1,2 @@\n+x\n+y\n\\ No newline at end of file\n");
            let patch = LineDiff::compute("x\n", "").unified_patch("x\n", "", "f", Some(3));
            assert_eq!(patch, "--- a/f\n+++ /dev/null\n@@ -1,1 +0,0 @@\n-x\n");
        }

        #[test]
        fn anchors_and_first_changed_line() {
            let diff = LineDiff::compute("1\n2\n3\n", "1\n3\nbaru\n");
            assert_eq!(diff.chunk_anchor(&diff.chunks[0]), (Side::Original, 2));
            assert_eq!(diff.chunk_anchor(&diff.chunks[1]), (Side::Modified, 3));
            assert_eq!(diff.first_changed_line(), Some(2));
            assert_eq!(LineDiff::compute("x\n", "").first_changed_line(), Some(1));
        }
    }
}
