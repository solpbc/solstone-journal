// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::Write;

fn main() {
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
        solstone_screen_ocr_helper::run_from_args,
    ));
    if !matches!(result, Ok(Ok(()))) {
        let mut stderr = std::io::stderr().lock();
        let _ = stderr.write_all(b"screen-ocr-helper: failed\n");
        let _ = stderr.flush();
        std::process::exit(1);
    }
}
