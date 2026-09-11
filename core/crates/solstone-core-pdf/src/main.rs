// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

fn main() {
    // PDFium's document/page object-graph traversal can recurse deeply enough
    // to overflow the default 1 MiB Windows main-thread stack (observed:
    // STATUS_STACK_OVERFLOW inspecting a plain two-page PDF on
    // WJL-HNBMKGDR). Unix default stacks (8 MiB+) have not shown this.
    // Run on a worker thread with a generous stack instead of trusting the
    // platform default.
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let code = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || solstone_core_pdf::entrypoint(args))
        .expect("spawn pdf worker thread")
        .join()
        .expect("pdf worker thread panicked");
    std::process::exit(code);
}
