// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import java.util.function.LongSupplier;

/** Temporary platform liveness only; native admission remains the owner. */
final class SetupForeground {
    static final long BUDGET_MS=15*60*1000;
    static final long CHECK_INTERVAL_MS=1000;
    interface Effects {
        boolean start() throws Exception;
        void finish();
    }
    private final LongSupplier clock;
    private String id;
    private long deadline;
    private Effects effects;
    SetupForeground(LongSupplier clock) { this.clock=clock; }
    boolean begin(String next, boolean admitted, Effects nextEffects) {
        expire();
        if (!admitted || next == null || next.isEmpty()) return false;
        close();
        id=next; effects=nextEffects; deadline=clock.getAsLong()+BUDGET_MS;
        try { if (nextEffects.start()) return true; }
        catch (Exception error) { /* Refusal has the same bounded cleanup. */ }
        close(); return false;
    }
    void end(String ended) {
        expire();
        if (id != null && id.equals(ended)) close();
    }
    void expire() { if (id != null && clock.getAsLong() >= deadline) close(); }
    long checkTimeout(String lease) {
        if (id == null || !id.equals(lease)) return 0;
        long remaining=deadline-clock.getAsLong();
        if (remaining <= 0) { close(); return 0; }
        return Math.min(remaining,CHECK_INTERVAL_MS);
    }
    void stop(Runnable cancelNative) {
        try { cancelNative.run(); }
        finally { close(); }
    }
    void close() {
        Effects retired=effects; id=null; effects=null;
        if (retired != null) retired.finish();
    }
}
