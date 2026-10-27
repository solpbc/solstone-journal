// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import android.content.ContentResolver;
import android.content.Intent;
import android.content.UriPermission;
import android.net.Uri;
import android.provider.DocumentsContract;

/** Constructed only by JournalService from its actual picker result. */
final class SourcePermissions {
    static SourcePermissionTransition forSelection(ContentResolver resolver,Uri selected,int flags) {
        String material=selected == null ? null : selected.toString();
        boolean persistable=(flags & Intent.FLAG_GRANT_READ_URI_PERMISSION) != 0
            && (flags & Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION) != 0;
        return new SourcePermissionTransition(material,persistable,new SourcePermissionTransition.Access() {
            private Uri tree(String value) {
                Uri uri=Uri.parse(value);
                if (!DocumentsContract.isTreeUri(uri)) throw new IllegalArgumentException("not a document tree");
                return uri;
            }
            public boolean hasRead(String value) {
                Uri uri=tree(value);
                for (UriPermission permission:resolver.getPersistedUriPermissions()) {
                    if (permission.isReadPermission() && permission.getUri().equals(uri)) return true;
                }
                return false;
            }
            public void takeRead(String value) {
                resolver.takePersistableUriPermission(tree(value),Intent.FLAG_GRANT_READ_URI_PERMISSION);
            }
            public void releaseRead(String value) {
                resolver.releasePersistableUriPermission(tree(value),Intent.FLAG_GRANT_READ_URI_PERMISSION);
            }
        },Native::folderCopyContinues);
    }
}
