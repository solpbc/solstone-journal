// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import android.content.Context;
import java.io.File;
import java.security.MessageDigest;
import java.util.Locale;

// The package record and library location come only from the signed app.
final class QwenRuntimeAssets {
    static String install(Context context) throws Exception {
        byte[] manifest = ServiceToolAssets.readAsset(context, "qwen-runtime/manifest.json");
        if (manifest.length > 64 * 1024) throw new java.io.IOException("local runtime record exceeds bound");
        StringBuilder identity = new StringBuilder();
        for (byte value : MessageDigest.getInstance("SHA-256").digest(manifest)) {
            identity.append(String.format(Locale.ROOT, "%02x", value & 255));
        }
        File base = ServiceToolAssets.directory(context.getFilesDir().getCanonicalFile(), "qwen-runtime");
        File resources = ServiceToolAssets.directory(base, identity.toString());
        ServiceToolAssets.materialize(resources, "manifest.json", manifest);
        return Native.admitQwenRuntime(
            new File(context.getApplicationInfo().nativeLibraryDir).getCanonicalPath(),
            new File(resources, "manifest.json").getCanonicalPath());
    }
}
