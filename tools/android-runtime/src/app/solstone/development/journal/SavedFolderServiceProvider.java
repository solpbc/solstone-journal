// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import android.content.ContentResolver;
import android.content.UriPermission;
import android.net.Uri;
import android.util.Log;
import android.provider.DocumentsContract;
import java.io.File;
import java.util.concurrent.atomic.AtomicBoolean;

/** Native saved-source callback. No Activity or document is required for a due sync. */
final class SavedFolderServiceProvider {
    private final ContentResolver resolver;
    SavedFolderServiceProvider(ContentResolver resolver) { this.resolver=resolver; }

    private static void copyLog(String stage, String outcome) {
        try { Log.i("JournalSource", "saved_folder_copy stage=" + stage + " outcome=" + outcome); }
        catch (RuntimeException unavailable) { }
    }

    private static String copyOutcome(String message) {
        switch (message) {
            case OwnerFolderCopy.READY: return "ready";
            case OwnerFolderCopy.CANCELLED: return "cancelled";
            case OwnerFolderCopy.DENIED: return "denied";
            case OwnerFolderCopy.UNAVAILABLE: return "unavailable";
            case OwnerFolderCopy.STORAGE_FULL: return "storage_full";
            case OwnerFolderCopy.TRUNCATED: return "changed";
            case OwnerFolderCopy.NO_MTIME: return "no_modified_time";
            case OwnerFolderCopy.SPECIAL: return "special_entry";
            case OwnerFolderCopy.ESCAPE: return "outside_tree";
            case OwnerFolderCopy.DUPLICATE: return "duplicate_name";
            default: return "other";
        }
    }

    private boolean permitted(Uri tree) {
        for (UriPermission permission:resolver.getPersistedUriPermissions()) {
            if (permission.isReadPermission() && permission.getUri().equals(tree)) return true;
        }
        return false;
    }

    // Called only through the registered native provider on the retained worker.
    // Identity and grant come from bound SyncState; full WorkStop is installed.
    int copySavedFolder(String identity,String material,String destination) {
        if (!Native.folderCopyContinues()) { copyLog("before_access", "stopped"); return 2; }
        try {
            Uri tree=Uri.parse(material);
            if (!DocumentsContract.isTreeUri(tree)) { copyLog("access", "invalid_tree"); return 1; }
            if (!permitted(tree)) { copyLog("access", "persisted_read_absent"); return 1; }
            copyLog("access", "persisted_read_confirmed");
            Thread worker=Thread.currentThread();
            OwnerFolderCopy.LiveAuthority live=new OwnerFolderCopy.LiveAuthority(identity,0,identity,0,
                () -> Thread.currentThread() == worker && Native.folderCopyContinues());
            OwnerFolderCopy.Result result=OwnerFolderCopy.copy(live,new OwnerTreeSource(resolver,tree),
                new File(destination),new AtomicBoolean(false));
            copyLog("java_return", result.authorityMatches ? copyOutcome(result.ownerMessage) : "authority_changed");
            if (result.authorityMatches && OwnerFolderCopy.READY.equals(result.ownerMessage)) return 0;
            // Provider wrappers deliberately collapse exceptions into a denied
            // copy. Re-read permission after failure so revocation during the
            // read becomes access loss, while Stop/transient failures do not.
            if (!Native.folderCopyContinues()) { copyLog("after_copy", "stopped"); return 2; }
            boolean retained = permitted(tree);
            copyLog("after_copy", retained ? "persisted_read_retained" : "persisted_read_lost");
            return retained ? 2 : 1;
        } catch (SecurityException lost) { copyLog("exception_access", lost.getClass().getName()); return 1; }
        catch (Exception unavailable) { copyLog("exception", unavailable.getClass().getName()); return 2; }
    }
}
