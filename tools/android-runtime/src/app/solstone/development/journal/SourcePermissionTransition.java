// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import java.util.function.BooleanSupplier;

/** One synchronous native configuration call's OS permission ownership. */
final class SourcePermissionTransition {
    interface Access {
        boolean hasRead(String material) throws Exception;
        void takeRead(String material) throws Exception;
        void releaseRead(String material) throws Exception;
    }

    private final String selected;
    private final boolean persistable;
    private final Access access;
    private final BooleanSupplier continues;
    private final Thread worker;
    private boolean added;

    SourcePermissionTransition(String selected, boolean persistable, Access access,
            BooleanSupplier continues) {
        this.selected=selected;
        this.persistable=persistable;
        this.access=access;
        this.continues=continues;
        this.worker=Thread.currentThread();
    }

    // The private native transaction owns its original Root, source exclusion
    // and full Stop through every callback and the actual cleanup return.
    // 0: acquire selected; 1: roll back newly acquired; 2: release retired;
    // 3: inspect a private stored grant. No material is returned to the page.
    int transition(int action, String material) {
        if (Thread.currentThread() != worker) return -1;
        switch (action) {
            case 0:
                if (selected == null || !selected.equals(material) || !continues.getAsBoolean()) return -1;
                try {
                    if (access.hasRead(selected)) return 0;
                    if (!persistable) return -1;
                    // A failed take may already have acquired the grant. This
                    // call knew it was absent; cleanup must inspect it too.
                    added=true;
                    access.takeRead(selected);
                    return continues.getAsBoolean() && access.hasRead(selected) ? 1 : -1;
                } catch (Exception unavailable) { return -1; }
            case 1:
                if (!added) return 0;
                if (release(selected) != 0) return -1;
                added=false;
                return 0;
            case 2:
                return release(material);
            case 3:
                try { return access.hasRead(material) ? 1 : 0; }
                catch (Exception unavailable) { return -1; }
            default:
                return -1;
        }
    }

    private int release(String material) {
        // Cleanup retires an already-owned permission even after Stop. Refusing
        // it merely because Stop arrived would leak the newly acquired grant.
        try {
            if (access.hasRead(material)) access.releaseRead(material);
            return access.hasRead(material) ? -1 : 0;
        } catch (Exception unavailable) { return -1; }
    }
}
