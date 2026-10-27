// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import android.content.ContentResolver;
import android.content.UriPermission;
import android.net.Uri;
import android.provider.DocumentsContract;
import java.io.File;
import java.util.concurrent.atomic.AtomicBoolean;

/** Native saved-source callback. No Activity or document is required for a due sync. */
final class SavedFolderServiceProvider {
    private final ContentResolver resolver;
    SavedFolderServiceProvider(ContentResolver resolver) { this.resolver=resolver; }

    private boolean permitted(Uri tree) {
        for (UriPermission permission:resolver.getPersistedUriPermissions()) {
            if (permission.isReadPermission() && permission.getUri().equals(tree)) return true;
        }
        return false;
    }

    // Called only through the registered native provider on the retained worker.
    // Identity and grant come from bound SyncState; full WorkStop is installed.
    int copySavedFolder(String identity,String material,String destination) {
        if (!Native.folderCopyContinues()) return 2;
        try {
            Uri tree=Uri.parse(material);
            if (!DocumentsContract.isTreeUri(tree)) return 1;
            if (!permitted(tree)) return 1;
            Thread worker=Thread.currentThread();
            OwnerFolderCopy.LiveAuthority live=new OwnerFolderCopy.LiveAuthority(identity,0,identity,0,
                () -> Thread.currentThread() == worker && Native.folderCopyContinues());
            OwnerFolderCopy.Result result=OwnerFolderCopy.copy(live,new OwnerTreeSource(resolver,tree),
                new File(destination),new AtomicBoolean(false));
            if (result.authorityMatches && OwnerFolderCopy.READY.equals(result.ownerMessage)) return 0;
            // Provider wrappers deliberately collapse exceptions into a denied
            // copy. Re-read permission after failure so revocation during the
            // read becomes access loss, while Stop/transient failures do not.
            if (!Native.folderCopyContinues()) return 2;
            return permitted(tree) ? 2 : 1;
        } catch (SecurityException lost) { return 1; }
        catch (Exception unavailable) { return 2; }
    }
}
