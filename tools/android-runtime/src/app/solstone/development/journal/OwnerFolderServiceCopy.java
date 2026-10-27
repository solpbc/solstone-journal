// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import android.content.ContentResolver;
import android.net.Uri;
import java.io.File;
import java.io.IOException;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.function.BooleanSupplier;

/** One synchronous callback owned by JournalService's retained request worker. */
final class OwnerFolderServiceCopy {
    private final ContentResolver resolver;
    private final Uri selected;
    private final long page;
    private final Thread worker;

    OwnerFolderServiceCopy(ContentResolver resolver, Uri selected, long page) {
        this.resolver = resolver;
        this.selected = selected;
        this.page = page;
        this.worker = Thread.currentThread();
    }

    // JNI invokes this only while with_folder_copy's full WorkStop is installed
    // on this same service thread. It retains no cursor, stream or destination.
    boolean copyTo(String destination) {
        if (Thread.currentThread() != worker) return false;
        try {
            final String authority = Long.toUnsignedString(page);
            final BooleanSupplier continues = () -> Thread.currentThread() == worker
                && Native.folderCopyContinues();
            OwnerFolderCopy.LiveAuthority live = new OwnerFolderCopy.LiveAuthority(
                authority, 0, Long.toUnsignedString(Native.folderPageCurrent()), 0, continues);
            OwnerFolderCopy.Result result = OwnerFolderCopy.copy(live,
                new OwnerTreeSource(resolver, selected), new File(destination), new AtomicBoolean(false));
            return result.authorityMatches && OwnerFolderCopy.READY.equals(result.ownerMessage);
        } catch (IOException | RuntimeException error) {
            return false;
        }
    }
}
