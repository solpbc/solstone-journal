// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import java.util.concurrent.Semaphore;
import java.util.concurrent.atomic.AtomicBoolean;

// One admitted request includes its executor handoff, execution and response.
// Stop closes admission before waiting for this owner; no old job can queue
// behind the active one and later adopt a replacement serving journal.
final class ThinkingAdmission {
    private final Semaphore active = new Semaphore(1);
    private long generation;
    private boolean open;

    synchronized long generation() { return generation; }
    synchronized boolean isOpen() { return open; }
    synchronized void opened(long expected) {
        if (generation == expected) open = true;
    }
    synchronized void stop() { open = false; generation++; }
    synchronized Ticket admit() {
        if (!open || !active.tryAcquire()) return null;
        return new Ticket(generation);
    }
    void awaitIdle() {
        active.acquireUninterruptibly();
        active.release();
    }
    final class Ticket implements AutoCloseable {
        private final long admittedGeneration;
        private final AtomicBoolean released = new AtomicBoolean();
        Ticket(long generation) { admittedGeneration = generation; }
        boolean current() {
            synchronized (ThinkingAdmission.this) {
                return !released.get() && open && generation == admittedGeneration;
            }
        }
        public void close() {
            if (released.compareAndSet(false, true)) active.release();
        }
    }
}
