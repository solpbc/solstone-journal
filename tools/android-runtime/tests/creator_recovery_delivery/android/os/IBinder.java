// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package android.os;
public interface IBinder {
    interface DeathRecipient { void binderDied(); }
    default void linkToDeath(DeathRecipient recipient,int flags) {}
    default boolean unlinkToDeath(DeathRecipient recipient,int flags) { return true; }
}
