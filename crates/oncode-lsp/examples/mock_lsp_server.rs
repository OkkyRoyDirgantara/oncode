//! Mock language server over stdio, for end-to-end tests (see `oncode_lsp::mock`).

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(oncode_lsp::mock::run_stdio(&args));
}
