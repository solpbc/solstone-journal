// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation

public enum JournalSetupCommand {
    /// `force` re-runs every step. Setup otherwise skips the steps its last run finished, and a
    /// skipped run leaves this installation bound to whichever journal it last set up.
    public static func setupArguments(journalURL: URL, skipService: Bool, force: Bool = false) -> [String] {
        var arguments = [
            "setup",
            "--jsonl",
            "--yes",
            "--skip-models",
            "--skip-wrapper",
            "--accept-existing-journal",
            "--journal",
            journalURL.path,
        ]
        if skipService {
            arguments.append("--skip-service")
        }
        if force {
            arguments.append("--force")
        }
        return arguments
    }
}
