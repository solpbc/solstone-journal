// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import java.util.HashMap;

/**
 * Admitted and completed upload replies share one bound. A finished reply
 * stays until finish takes it, cancel drops it, or the page retires it.
 */
final class OwnerUploadBoard {
    static final int BOUND = 32;

    static final class Taken {
        final boolean pending;
        final String reply;

        private Taken(boolean pending, String reply) {
            this.pending = pending;
            this.reply = reply;
        }

        static Taken pending() { return new Taken(true, null); }
        static Taken absent() { return new Taken(false, null); }
        static Taken ready(String reply) { return new Taken(false, reply); }
    }

    private static final class Slot {
        String reply;
    }

    private final HashMap<String, Slot> slots = new HashMap<String, Slot>();

    synchronized boolean admit(String key) {
        if (key == null || key.isEmpty() || slots.containsKey(key) || slots.size() >= BOUND) return false;
        slots.put(key, new Slot());
        return true;
    }

    synchronized boolean keep(String key, String reply) {
        Slot slot = slots.get(key);
        if (slot == null || reply == null) return false;
        if (slot.reply == null) slot.reply = reply;
        return true;
    }

    synchronized Taken take(String key) {
        Slot slot = slots.get(key);
        if (slot == null) return Taken.absent();
        if (slot.reply == null) return Taken.pending();
        slots.remove(key);
        return Taken.ready(slot.reply);
    }

    synchronized void cancel(String key) { slots.remove(key); }

    synchronized void retire() { slots.clear(); }

    synchronized int held() { return slots.size(); }
}
