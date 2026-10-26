// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;
public final class Native {
    static { System.loadLibrary("journalbridge"); }
    public static native String run(String root, String op, int count);
    static native byte[] openOwner(String root, byte[] request, int writeDescriptor);
    static native byte[] openOwnerStream(String root, byte[] request, int responseDescriptor, int bodyDescriptor);
    static native byte[] pollOwner(long id);
    static native byte[] reclaimBridge(String cache, String journal, String liveJson);
    static native void cancelOwner(long id);
    // Called only by first-party Service startup with app metadata and APK assets.
    static native String admitServiceTools(String nativeDirectory, String resourceRoot,
                                          byte[] manifest, byte[] buildReceipt);
}
