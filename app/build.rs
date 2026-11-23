// Menyematkan ikon aplikasi ke oncode.exe. Target diperiksa saat build script berjalan
// (bukan lewat cfg) agar kompilasi silang dari Linux ke Windows ikut mendapat ikon.
fn main() {
    println!("cargo:rerun-if-changed=icons/oncode.rc");
    println!("cargo:rerun-if-changed=icons/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("icons/oncode.rc", embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
