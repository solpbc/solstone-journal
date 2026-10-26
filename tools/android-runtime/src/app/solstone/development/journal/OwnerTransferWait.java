// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

/** Input silence stalls. Time after the body is closed does not. */
final class OwnerTransferWait {
    private OwnerTransferWait() {}

    static boolean stalled(boolean bodyClosed, long elapsedMs) {
        return !bodyClosed && elapsedMs > 60000L;
    }
}
