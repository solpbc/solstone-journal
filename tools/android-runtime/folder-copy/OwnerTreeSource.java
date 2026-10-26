// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import android.content.ContentResolver;
import android.database.Cursor;
import android.net.Uri;
import android.os.Bundle;
import android.provider.DocumentsContract;
import java.io.IOException;
import java.io.InputStream;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.List;

/** Reads logical documents only through an already granted picker tree. */
final class OwnerTreeSource implements OwnerFolderCopy.FolderSource {
    private static final String[] FACTS = {
        DocumentsContract.Document.COLUMN_DOCUMENT_ID,
        DocumentsContract.Document.COLUMN_DISPLAY_NAME,
        DocumentsContract.Document.COLUMN_MIME_TYPE,
        DocumentsContract.Document.COLUMN_SIZE,
        DocumentsContract.Document.COLUMN_LAST_MODIFIED,
        DocumentsContract.Document.COLUMN_FLAGS
    };
    private final ContentResolver resolver;
    private final Uri tree;
    private final String rootId;
    // Only metadata is cached, never file contents or an open provider cursor.
    private final LinkedHashMap<String, Map<String, Document>> listings =
            new LinkedHashMap<String, Map<String, Document>>(16, 0.75f, true) {
                protected boolean removeEldestEntry(Map.Entry<String, Map<String, Document>> entry) {
                    return size() > 8;
                }
            };

    OwnerTreeSource(ContentResolver resolver, Uri pickedTree) throws IOException {
        if (resolver == null || pickedTree == null || !"content".equals(pickedTree.getScheme())
                || !DocumentsContract.isTreeUri(pickedTree)) {
            throw new IOException("invalid folder grant");
        }
        this.resolver = resolver;
        this.tree = pickedTree;
        try { this.rootId = DocumentsContract.getTreeDocumentId(pickedTree); }
        catch (RuntimeException error) { throw new IOException("invalid folder grant", error); }
        if (rootId == null || rootId.isEmpty()) { throw new IOException("invalid folder grant"); }
    }

    private static final class Document {
        final String id, name, mime;
        final Long modified;
        final long bytes;
        final int flags;
        Document(Cursor cursor) throws IOException {
            if (cursor.isNull(0) || cursor.isNull(1) || cursor.isNull(2) || cursor.isNull(5)) {
                throw new IOException("folder metadata unavailable");
            }
            id = cursor.getString(0);
            name = cursor.getString(1);
            mime = cursor.getString(2);
            bytes = cursor.isNull(3) ? -1 : cursor.getLong(3);
            modified = cursor.isNull(4) ? null : cursor.getLong(4);
            flags = cursor.getInt(5);
            if (id == null || id.isEmpty() || bytes < -1) { throw new IOException("folder metadata unavailable"); }
        }
        boolean directory() { return DocumentsContract.Document.MIME_TYPE_DIR.equals(mime); }
        boolean unavailable() {
            return (flags & (DocumentsContract.Document.FLAG_VIRTUAL_DOCUMENT
                    | DocumentsContract.Document.FLAG_PARTIAL)) != 0;
        }
    }

    private Document query(String id) throws IOException {
        Uri uri = DocumentsContract.buildDocumentUriUsingTree(tree, id);
        try (Cursor cursor = resolver.query(uri, FACTS, null, null, null)) {
            if (cursor == null || !cursor.moveToFirst()) { throw new IOException("folder unavailable"); }
            requireComplete(cursor);
            Document result = new Document(cursor);
            if (!id.equals(result.id) || cursor.moveToNext()) { throw new IOException("folder changed"); }
            requireComplete(cursor);
            return result;
        } catch (RuntimeException error) {
            throw new IOException("folder unavailable", error);
        }
    }

    private ArrayList<Document> childrenOf(String id) throws IOException {
        Uri children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, id);
        ArrayList<Document> result = new ArrayList<Document>();
        HashSet<String> names = new HashSet<String>();
        try (Cursor cursor = resolver.query(children, FACTS, null, null, null)) {
            if (cursor == null) { throw new IOException("folder unavailable"); }
            requireComplete(cursor);
            while (cursor.moveToNext()) {
                Document child = new Document(cursor);
                if (!component(child.name) || !names.add(child.name)) { throw new IOException("folder changed"); }
                result.add(child);
            }
            requireComplete(cursor);
        } catch (RuntimeException error) {
            throw new IOException("folder unavailable", error);
        }
        HashMap<String, Document> indexed = new HashMap<String, Document>();
        for (Document child : result) { indexed.put(child.name, child); }
        listings.put(id, indexed);
        return result;
    }

    private static void requireComplete(Cursor cursor) throws IOException {
        Bundle extras = cursor.getExtras();
        if (extras != null && (extras.getBoolean(DocumentsContract.EXTRA_LOADING, false)
                || extras.getString(DocumentsContract.EXTRA_ERROR) != null)) {
            throw new IOException("folder unavailable");
        }
    }

    private void verifyMembership(String parent, Document child) throws IOException {
        try {
            DocumentsContract.Path found = DocumentsContract.findDocumentPath(
                    resolver, DocumentsContract.buildDocumentUriUsingTree(tree, child.id));
            if (found != null) {
                List<String> path = found.getPath();
                if (path.size() >= 2 && rootId.equals(path.get(0))
                        && parent.equals(path.get(path.size() - 2))
                        && child.id.equals(path.get(path.size() - 1))) {
                    return;
                }
                // A document may have several parents; its canonical route
                // need not be this one. Re-list this parent before refusing.
            }
        } catch (UnsupportedOperationException unsupported) {
            // Providers without path support must re-list the immediate parent.
        } catch (RuntimeException error) { throw new IOException("folder unavailable", error); }
        for (Document current : childrenOf(parent)) {
            if (current.name.equals(child.name) && current.id.equals(child.id)) { return; }
        }
        throw new IOException("folder changed");
    }

    private Document childNamed(String parent, String name, boolean verify) throws IOException {
        Map<String, Document> children = listings.get(parent);
        if (children == null) {
            childrenOf(parent);
            children = listings.get(parent);
        }
        Document child = children.get(name);
        if (child == null) { throw new IOException("folder changed"); }
        Document current = query(child.id);
        if (!current.name.equals(name)) { throw new IOException("folder changed"); }
        if (verify) { verifyMembership(parent, current); }
        return current;
    }

    // Document ids are opaque: resolve display-name components by querying each
    // parent, never by constructing an id or accepting a filesystem path.
    private Document resolve(String relative) throws IOException {
        return resolve(relative, false);
    }

    private Document resolve(String relative, boolean verify) throws IOException {
        Document current = query(rootId);
        HashSet<String> ancestors = new HashSet<String>();
        ancestors.add(rootId);
        if (relative.isEmpty()) { return current; }
        for (String name : relative.split("/", -1)) {
            if (!component(name) || !current.directory()) { throw new IOException("invalid folder entry"); }
            Document found = childNamed(current.id, name, verify);
            if (!ancestors.add(found.id)) { throw new IOException("folder changed"); }
            current = found;
        }
        return current;
    }

    public String[] children(String relative) throws IOException {
        Document parent = resolve(relative, true);
        if (!parent.directory() || parent.unavailable()) { throw new IOException("folder unavailable"); }
        ArrayList<Document> children = childrenOf(parent.id);
        ArrayList<String> result = new ArrayList<String>();
        for (Document child : children) {
            if (child.id.equals(rootId) || child.id.equals(parent.id)) { throw new IOException("folder changed"); }
            result.add(child.name);
        }
        return result.toArray(new String[result.size()]);
    }
    public InputStream open(String relative) throws IOException {
        Document document = resolve(relative, true);
        if (document.directory() || document.unavailable()) { throw new IOException("folder unavailable"); }
        try {
            InputStream stream = resolver.openInputStream(DocumentsContract.buildDocumentUriUsingTree(tree, document.id));
            if (stream == null) { throw new IOException("folder unavailable"); }
            return stream;
        } catch (RuntimeException error) { throw new IOException("folder unavailable", error); }
    }
    public long size(String relative) throws IOException { return resolve(relative, true).bytes; }
    public boolean directory(String relative) throws IOException { return resolve(relative).directory(); }
    public boolean symlink(String relative) throws IOException { return resolve(relative).unavailable(); }
    public Long modifiedMillis(String relative) throws IOException { return resolve(relative, true).modified; }

    private static boolean component(String name) {
        return name != null && !name.isEmpty() && !name.equals(".") && !name.equals("..")
                && name.indexOf('/') < 0 && name.indexOf('\\') < 0 && name.indexOf('\0') < 0;
    }
}
