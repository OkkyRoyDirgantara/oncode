//! Memastikan semua tipe yang menyeberang ke UI bisa diekspor ke TypeScript.
//!
//! Ekspor gagal bila ada tipe berukuran BigInt (i64/u64/usize/isize), sehingga tes ini
//! sekaligus menjaga aturan "tanpa bilangan bulat besar" pada tipe UI.

use oncode_forge::{
    Account, Check, CheckState, DeviceCode, ForgeError, ForgeErrorCode, ForgeHost, ForgeKind, NewPullRequest, Page,
    PrFilter, PrState, PullRequest, PullRequestDetail, RemoteRepo, RepoRef,
};
use specta::Types;

#[derive(specta::Type)]
#[allow(dead_code)]
struct Pages {
    repos: Page<RemoteRepo>,
    pull_requests: Page<PullRequest>,
}

fn export() -> String {
    let types = Types::default()
        .register::<ForgeErrorCode>()
        .register::<ForgeError>()
        .register::<ForgeKind>()
        .register::<ForgeHost>()
        .register::<RepoRef>()
        .register::<Account>()
        .register::<RemoteRepo>()
        .register::<PrState>()
        .register::<PullRequest>()
        .register::<PullRequestDetail>()
        .register::<CheckState>()
        .register::<Check>()
        .register::<PrFilter>()
        .register::<NewPullRequest>()
        .register::<DeviceCode>()
        .register::<Pages>();
    specta_typescript::Typescript::default().export(&types, specta_serde::Format).expect("ekspor TypeScript")
}

#[test]
fn ui_types_export_to_typescript() {
    let ts = export();
    println!("{ts}");

    // Nama varian mengikuti serde.
    assert!(ts.contains("\"github\"") && ts.contains("\"gitlab\""), "{ts}");
    assert!(ts.contains("\"reviewRequested\""), "{ts}");
    assert!(ts.contains("\"rateLimited\""), "{ts}");
    // Field camelCase.
    for field in ["cloneUrlHttps", "defaultBranch", "headSha", "isCrossRepo", "changedFiles", "nextPage", "verificationUriComplete"] {
        assert!(ts.contains(field), "field {field} tidak ada:\n{ts}");
    }
    // `Page<T>` tetap generik di sisi TypeScript.
    assert!(ts.contains("Page<T>"), "{ts}");
    assert!(ts.contains("Page<RemoteRepo>") && ts.contains("Page<PullRequest>"), "{ts}");
    // Tidak ada tipe yang bocor sebagai bigint.
    assert!(!ts.contains("bigint"), "{ts}");
}
