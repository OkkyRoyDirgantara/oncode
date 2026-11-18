//! Pembingkaian protokol dasar LSP: `Content-Length: N\r\n\r\n<N byte JSON>`.

/// Batas ukuran satu blok header. Lebih dari ini dianggap sampah.
const MAX_HEADER_BYTES: usize = 64 * 1024;
/// Batas ukuran satu isi pesan.
const MAX_BODY_BYTES: usize = 256 * 1024 * 1024;
const CONTENT_LENGTH: &[u8] = b"content-length";

/// Membingkai satu pesan untuk dikirim ke server.
pub(crate) fn encode_frame(json: &str) -> Vec<u8> {
    let header = format!("Content-Length: {}\r\n\r\n", json.len());
    let mut frame = Vec::with_capacity(header.len() + json.len());
    frame.extend_from_slice(header.as_bytes());
    frame.extend_from_slice(json.as_bytes());
    frame
}

/// Pengurai inkremental protokol dasar LSP: masukkan byte seadanya, terima isi pesan yang sudah lengkap.
///
/// Toleran terhadap keluaran yang tidak rapi: header tambahan diabaikan, nama header tidak peka huruf
/// besar, baris boleh diakhiri `\n` saja, dan blok header yang rusak (tanpa `Content-Length` yang sah,
/// lebih dari 64 KiB, atau menyatakan isi lebih dari 256 MiB) dibuang lalu pencarian dilanjutkan
/// ke header berikutnya.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
    /// Panjang isi yang sedang ditunggu; `None` berarti sedang mencari header.
    body_len: Option<usize>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Menambahkan byte yang baru dibaca dan mengembalikan semua isi pesan (teks JSON tanpa header)
    /// yang kini lengkap, sesuai urutan.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(bytes);
        let mut messages = Vec::new();
        let mut pos = 0;
        loop {
            let rest = &self.buf[pos..];
            if let Some(len) = self.body_len {
                if rest.len() < len {
                    break;
                }
                if len > 0 {
                    messages.push(String::from_utf8_lossy(&rest[..len]).into_owned());
                }
                pos += len;
                self.body_len = None;
                continue;
            }
            let Some((block_len, consumed)) = find_blank_line(rest) else {
                // Belum ada akhir header. Header yang sah tidak mungkin dimulai lebih dari
                // MAX_HEADER_BYTES sebelum akhir buffer, jadi sisanya aman dibuang.
                pos += rest.len().saturating_sub(MAX_HEADER_BYTES);
                break;
            };
            // Sampah di depan header yang kepanjangan dipotong; header aslinya ada di dekat baris kosong.
            let block = &rest[block_len.saturating_sub(MAX_HEADER_BYTES)..block_len];
            self.body_len = parse_content_length(block).filter(|len| *len <= MAX_BODY_BYTES);
            pos += consumed;
        }
        self.buf.drain(..pos);
        messages
    }
}

/// Mencari baris kosong penutup header. Mengembalikan `(panjang blok header, jumlah byte termasuk penutup)`.
fn find_blank_line(buf: &[u8]) -> Option<(usize, usize)> {
    let mut from = 0;
    while let Some(offset) = buf[from..].iter().position(|byte| *byte == b'\n') {
        let newline = from + offset;
        match buf.get(newline + 1) {
            Some(b'\n') => return Some((newline, newline + 2)),
            Some(b'\r') if buf.get(newline + 2) == Some(&b'\n') => return Some((newline, newline + 3)),
            _ => from = newline + 1,
        }
    }
    None
}

/// Mengambil nilai `Content-Length` dari satu blok header. Baris yang paling dekat dengan
/// akhir blok diutamakan; bila tidak ada baris yang rapi, dicoba mencari di tengah baris
/// (mis. sisa isi pesan sebelumnya menempel di depan header).
fn parse_content_length(block: &[u8]) -> Option<usize> {
    let lines = || block.split(|byte| *byte == b'\n').rev();
    lines().find_map(exact_content_length).or_else(|| lines().find_map(embedded_content_length))
}

fn exact_content_length(line: &[u8]) -> Option<usize> {
    let colon = line.iter().position(|byte| *byte == b':')?;
    if !line[..colon].trim_ascii().eq_ignore_ascii_case(CONTENT_LENGTH) {
        return None;
    }
    parse_decimal(&line[colon + 1..])
}

fn embedded_content_length(line: &[u8]) -> Option<usize> {
    let start = line
        .windows(CONTENT_LENGTH.len())
        .rposition(|window| window.eq_ignore_ascii_case(CONTENT_LENGTH))?;
    let after = line[start + CONTENT_LENGTH.len()..].trim_ascii_start();
    parse_decimal(after.strip_prefix(b":")?)
}

fn parse_decimal(value: &[u8]) -> Option<usize> {
    let value = value.trim_ascii();
    if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(value).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(body: &str) -> Vec<u8> {
        encode_frame(body)
    }

    #[test]
    fn encodes_byte_length_not_char_count() {
        assert_eq!(encode_frame("{}"), b"Content-Length: 2\r\n\r\n{}");
        assert_eq!(encode_frame("\"é\""), "Content-Length: 4\r\n\r\n\"é\"".as_bytes());
    }

    #[test]
    fn decodes_a_single_message() {
        let mut decoder = FrameDecoder::new();
        assert_eq!(decoder.push(&frame(r#"{"jsonrpc":"2.0","id":1}"#)), vec![r#"{"jsonrpc":"2.0","id":1}"#]);
        assert!(decoder.push(b"").is_empty());
    }

    #[test]
    fn header_split_across_reads() {
        let bytes = frame(r#"{"a":1}"#);
        // Semua titik potong, termasuk di tengah "Content-Length" dan di tengah "\r\n\r\n".
        for cut in 0..=bytes.len() {
            let mut decoder = FrameDecoder::new();
            let mut out = decoder.push(&bytes[..cut]);
            out.extend(decoder.push(&bytes[cut..]));
            assert_eq!(out, vec![r#"{"a":1}"#], "potongan di {cut}");
        }
    }

    #[test]
    fn byte_at_a_time() {
        let mut bytes = frame(r#"{"a":"satu"}"#);
        bytes.extend(frame(r#"{"b":"dua"}"#));
        let mut decoder = FrameDecoder::new();
        let mut out = Vec::new();
        for byte in &bytes {
            out.extend(decoder.push(std::slice::from_ref(byte)));
        }
        assert_eq!(out, vec![r#"{"a":"satu"}"#, r#"{"b":"dua"}"#]);
    }

    #[test]
    fn multiple_messages_in_one_read() {
        let mut bytes = frame("{\"n\":1}");
        bytes.extend(frame("{\"n\":2}"));
        bytes.extend(frame("{\"n\":3}"));
        // Pesan keempat baru sebagian.
        let fourth = frame("{\"n\":4}");
        bytes.extend(&fourth[..fourth.len() - 3]);

        let mut decoder = FrameDecoder::new();
        assert_eq!(decoder.push(&bytes), vec!["{\"n\":1}", "{\"n\":2}", "{\"n\":3}"]);
        assert_eq!(decoder.push(&fourth[fourth.len() - 3..]), vec!["{\"n\":4}"]);
    }

    #[test]
    fn extra_headers_are_ignored() {
        let mut decoder = FrameDecoder::new();
        let body = r#"{"ok":true}"#;
        let bytes = format!(
            "Content-Type: application/vscode-jsonrpc; charset=utf-8\r\nContent-Length: {}\r\nX-Lain: 1\r\n\r\n{body}",
            body.len()
        );
        assert_eq!(decoder.push(bytes.as_bytes()), vec![body]);
    }

    #[test]
    fn header_names_are_case_insensitive() {
        for header in ["content-length: 2", "CONTENT-LENGTH:2", "Content-length :  2  ", "cOnTeNt-LeNgTh:\t2"] {
            let mut decoder = FrameDecoder::new();
            let bytes = format!("{header}\r\n\r\n{{}}");
            assert_eq!(decoder.push(bytes.as_bytes()), vec!["{}"], "header {header:?}");
        }
    }

    #[test]
    fn bare_newlines_are_accepted() {
        let mut decoder = FrameDecoder::new();
        assert_eq!(decoder.push(b"Content-Length: 2\n\n{}Content-Length: 2\r\n\n[]"), vec!["{}", "[]"]);
    }

    #[test]
    fn multibyte_body_split_across_reads() {
        let body = r#"{"teks":"héllo – 日本語 🚀"}"#;
        let bytes = frame(body);
        assert!(body.len() > body.chars().count());
        for cut in 0..=bytes.len() {
            let mut decoder = FrameDecoder::new();
            let mut out = decoder.push(&bytes[..cut]);
            out.extend(decoder.push(&bytes[cut..]));
            assert_eq!(out, vec![body], "potongan di {cut}");
        }
    }

    #[test]
    fn body_may_contain_header_like_text() {
        let body = "{\"x\":\"Content-Length: 999\\r\\n\\r\\n\"}\r\n\r\nContent-Length: 1\r\n\r\n";
        let mut bytes = frame(body);
        bytes.extend(frame("{}"));
        let mut decoder = FrameDecoder::new();
        assert_eq!(decoder.push(&bytes), vec![body, "{}"]);
    }

    #[test]
    fn empty_body_is_skipped() {
        let mut decoder = FrameDecoder::new();
        assert_eq!(decoder.push(b"Content-Length: 0\r\n\r\nContent-Length: 2\r\n\r\n{}"), vec!["{}"]);
    }

    #[test]
    fn invalid_utf8_body_is_replaced_not_dropped() {
        let mut decoder = FrameDecoder::new();
        let out = decoder.push(b"Content-Length: 3\r\n\r\n\"\xff\"");
        assert_eq!(out, vec!["\"\u{fffd}\""]);
    }

    #[test]
    fn malformed_header_block_is_skipped() {
        // Nilai bukan angka, tanpa Content-Length, dan angka negatif: semuanya dibuang.
        for bad in ["Content-Length: abc\r\n\r\n", "Content-Type: x\r\n\r\n", "Content-Length: -5\r\n\r\n", "\r\n\r\n", "sampah\n\n"] {
            let mut decoder = FrameDecoder::new();
            let mut bytes = bad.as_bytes().to_vec();
            bytes.extend(frame("{\"ok\":1}"));
            assert_eq!(decoder.push(&bytes), vec!["{\"ok\":1}"], "blok rusak {bad:?}");
        }
    }

    #[test]
    fn junk_before_a_header_is_tolerated() {
        // Log yang nyasar ke stdout sebelum pesan pertama.
        let mut decoder = FrameDecoder::new();
        let mut bytes = b"Server mulai...\nmemuat indeks\n".to_vec();
        bytes.extend(frame("{\"a\":1}"));
        assert_eq!(decoder.push(&bytes), vec!["{\"a\":1}"]);

        // Sisa isi pesan rusak menempel di baris header berikutnya.
        let mut decoder = FrameDecoder::new();
        let mut bytes = b"Content-Length: x\r\n\r\n{\"rusak\":true}".to_vec();
        bytes.extend(frame("{\"b\":2}"));
        bytes.extend(frame("{\"c\":3}"));
        assert_eq!(decoder.push(&bytes), vec!["{\"b\":2}", "{\"c\":3}"]);

        // Baris tambahan setelah isi pesan.
        let mut decoder = FrameDecoder::new();
        assert_eq!(decoder.push(b"Content-Length: 2\r\n\r\n{}\r\nContent-Length: 2\r\n\r\n[]"), vec!["{}", "[]"]);
    }

    #[test]
    fn oversized_header_is_dropped_without_unbounded_growth() {
        let mut decoder = FrameDecoder::new();
        // 1 MiB sampah tanpa baris kosong, dikirim sedikit demi sedikit.
        let junk = vec![b'x'; 16 * 1024];
        for _ in 0..64 {
            assert!(decoder.push(&junk).is_empty());
            assert!(decoder.buf.len() <= MAX_HEADER_BYTES, "buffer tumbuh: {}", decoder.buf.len());
        }
        // Pesan sah setelahnya tetap terbaca (sampah menempel di depan baris header).
        assert_eq!(decoder.push(&frame("{\"selamat\":true}")), vec!["{\"selamat\":true}"]);
        assert!(decoder.buf.is_empty());
    }

    #[test]
    fn oversized_header_with_newlines_is_recovered() {
        let mut decoder = FrameDecoder::new();
        let mut bytes = Vec::new();
        while bytes.len() < 3 * MAX_HEADER_BYTES {
            bytes.extend_from_slice(b"baris log tanpa arti\n");
        }
        bytes.extend(frame("{\"n\":1}"));
        bytes.extend(frame("{\"n\":2}"));
        assert_eq!(decoder.push(&bytes), vec!["{\"n\":1}", "{\"n\":2}"]);
    }

    #[test]
    fn oversized_body_is_rejected() {
        let mut decoder = FrameDecoder::new();
        let too_big = MAX_BODY_BYTES + 1;
        let mut bytes = format!("Content-Length: {too_big}\r\n\r\n").into_bytes();
        bytes.extend_from_slice(b"Content-Length: 99999999999999999999999999\r\n\r\n");
        bytes.extend(frame("{\"ok\":1}"));
        assert_eq!(decoder.push(&bytes), vec!["{\"ok\":1}"]);
        assert!(decoder.buf.is_empty());
    }

    #[test]
    fn last_content_length_in_a_block_wins() {
        let mut decoder = FrameDecoder::new();
        let out = decoder.push(b"log: Content-Length: 99 ditolak\nContent-Length: 2\r\n\r\n{}");
        assert_eq!(out, vec!["{}"]);
    }

    #[test]
    fn large_body_in_small_chunks() {
        let body = format!("{{\"data\":\"{}\"}}", "a".repeat(300_000));
        let bytes = frame(&body);
        let mut decoder = FrameDecoder::new();
        let mut out = Vec::new();
        for chunk in bytes.chunks(4096) {
            out.extend(decoder.push(chunk));
        }
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], body);
    }
}
