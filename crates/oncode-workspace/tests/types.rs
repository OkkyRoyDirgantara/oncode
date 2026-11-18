//! Memastikan semua tipe yang menyeberang ke UI bisa diekspor ke TypeScript.
//!
//! Ekspor gagal bila ada tipe berukuran BigInt (i64/u64/usize/isize), sehingga tes ini
//! sekaligus menjaga aturan "tanpa bilangan bulat besar" pada tipe UI.

use oncode_workspace::{
    DirEntry, Eol, FileMatches, FileStat, LineMatch, OpenedFile, QuickOpenItem, ReplaceSummary, SearchQuery,
    SearchSummary, TextFile, WatchEvent, WsError, WsErrorCode,
};
use specta::Types;

#[test]
fn ui_types_export_to_typescript() {
    let types = Types::default()
        .register::<WsErrorCode>()
        .register::<WsError>()
        .register::<DirEntry>()
        .register::<Eol>()
        .register::<FileStat>()
        .register::<TextFile>()
        .register::<OpenedFile>()
        .register::<WatchEvent>()
        .register::<SearchQuery>()
        .register::<LineMatch>()
        .register::<FileMatches>()
        .register::<SearchSummary>()
        .register::<ReplaceSummary>()
        .register::<QuickOpenItem>();
    let ts = specta_typescript::Typescript::default().export(&types, specta_serde::Format).expect("ekspor TypeScript");
    println!("{ts}");

    // Nama varian mengikuti serde.
    for variant in ["\"invalidPattern\"", "\"permissionDenied\"", "\"crlf\"", "\"tooLarge\"", "\"binary\"", "\"text\"", "\"files\"", "\"git\""] {
        assert!(ts.contains(variant), "varian {variant} tidak ada:\n{ts}");
    }
    // Field camelCase.
    for field in ["isDir", "isSymlink", "mtimeMs", "readOnly", "gitDir", "isRegex", "caseSensitive", "wholeWord", "maxResults", "previewOffset"] {
        assert!(ts.contains(field), "field {field} tidak ada:\n{ts}");
    }
    assert!(ts.contains("{ kind: \"git\"; gitDir: string; refs: boolean; index: boolean; state: boolean }"), "{ts}");
    // Tidak ada tipe yang bocor sebagai bigint.
    assert!(!ts.contains("bigint"), "{ts}");
}
