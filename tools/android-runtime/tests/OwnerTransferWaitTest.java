// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

public final class OwnerTransferWaitTest {
    public static void main(String[] args) {
        if (!OwnerTransferWait.stalled(false, 60001L)) throw new AssertionError("silent input stalls");
        if (OwnerTransferWait.stalled(false, 60000L)) throw new AssertionError("input stall waits out the full interval");
        if (OwnerTransferWait.stalled(true, 120000L)) throw new AssertionError("save work after EOF is not a stall");
        System.out.println("owner transfer wait passed");
    }
}
