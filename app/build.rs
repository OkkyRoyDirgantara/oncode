// Embeds the application icon into oncode-app.exe. The target is checked while the build script runs
// (not via cfg) so that cross-compiling from Linux to Windows also gets the icon.
fn main() {
    println!("cargo:rerun-if-changed=icons/oncode.rc");
    println!("cargo:rerun-if-changed=icons/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("icons/oncode.rc", embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
