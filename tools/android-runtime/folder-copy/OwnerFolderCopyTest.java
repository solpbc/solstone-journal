// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import java.io.ByteArrayInputStream;
import java.io.File;
import java.io.InputStream;
import java.nio.file.Files;
import java.util.HashMap;
import java.util.Map;
import java.util.concurrent.atomic.AtomicBoolean;

public final class OwnerFolderCopyTest {
    static void require(boolean value) {
        if (!value) {
            throw new AssertionError();
        }
    }

    public static void main(String[] args) throws Exception {
        nestedBytesAndMtime();
        fileLargerThanTheBuffer();
        symlinkEscapeDuplicateAndMissingMtime();
        detachedPathAndBookmark();
        authorityMismatch();
        cancel();
        sameGrant();
        custodyAndSiblingStayWhenCopyFails();
        prefixOccupiedEmptySymlinkAndLiveAuthority();
        System.out.println("owner folder copy passed");
    }

    static void nestedBytesAndMtime() throws Exception {
        MemoryFolder source = new MemoryFolder();
        source.dir("");
        source.dir("notes");
        source.dir("notes/sub");
        source.file("notes/a.md", "hello".getBytes("UTF-8"), 1_700_000_000_000L);
        source.file("notes/sub/b.md", "world!!!!!".getBytes("UTF-8"), 1_700_000_111_000L);
        File dest = Files.createTempDirectory("folder-copy").toFile();
        dest.delete();
        OwnerFolderCopy.Result result = OwnerFolderCopy.copy("grant", 1, "grant", 1, source, dest, new AtomicBoolean());
        require(result.authorityMatches);
        require(OwnerFolderCopy.READY.equals(result.ownerMessage));
        require(bytes(new File(dest, "notes/a.md")).equals("hello"));
        require(bytes(new File(dest, "notes/sub/b.md")).equals("world!!!!!"));
        require(new File(dest, "notes/a.md").lastModified() / 1000L == 1_700_000_000L);
        require(new File(dest, "notes/sub/b.md").lastModified() / 1000L == 1_700_000_111L);
        delete(dest);
    }

    static void fileLargerThanTheBuffer() throws Exception {
        byte[] body = new byte[OwnerFolderCopy.BUFFER_BYTES + 3];
        for (int index = 0; index < body.length; index++) {
            body[index] = (byte) (index & 0xff);
        }
        MemoryFolder source = new MemoryFolder();
        source.dir("");
        source.file("wide.bin", body, 1_700_000_000_000L);
        File dest = Files.createTempDirectory("folder-copy-wide").toFile();
        dest.delete();
        OwnerFolderCopy.Result result = OwnerFolderCopy.copy("grant", 1, "grant", 1, source, dest, new AtomicBoolean());
        require(OwnerFolderCopy.READY.equals(result.ownerMessage));
        byte[] copied = Files.readAllBytes(new File(dest, "wide.bin").toPath());
        require(copied.length == body.length);
        require(copied[OwnerFolderCopy.BUFFER_BYTES] == body[OwnerFolderCopy.BUFFER_BYTES]);
        delete(dest);
    }

    static void symlinkEscapeDuplicateAndMissingMtime() {
        File dest = new File(temp("refused"), "out");
        MemoryFolder link = new MemoryFolder();
        link.dir("");
        link.file("link", new byte[] {1}, 1L);
        link.symlinks.add("link");
        OwnerFolderCopy.Result linked = OwnerFolderCopy.copy("grant", 1, "grant", 1, link, dest, new AtomicBoolean());
        require(linked.authorityMatches);
        require(OwnerFolderCopy.SPECIAL.equals(linked.ownerMessage));
        require(!dest.exists());

        MemoryFolder escape = new MemoryFolder();
        escape.dir("");
        escape.names.put("", new String[] {"../outside"});
        OwnerFolderCopy.Result outside = OwnerFolderCopy.copy("grant", 1, "grant", 1, escape, dest, new AtomicBoolean());
        require(OwnerFolderCopy.ESCAPE.equals(outside.ownerMessage));
        require(!dest.exists());

        MemoryFolder duplicate = new MemoryFolder();
        duplicate.dir("");
        duplicate.names.put("", new String[] {"a.md", "a.md"});
        duplicate.file("a.md", new byte[] {1}, 1L);
        OwnerFolderCopy.Result same = OwnerFolderCopy.copy("grant", 1, "grant", 1, duplicate, dest, new AtomicBoolean());
        require(OwnerFolderCopy.DUPLICATE.equals(same.ownerMessage));
        require(!dest.exists());

        MemoryFolder missing = new MemoryFolder();
        missing.dir("");
        missing.file("a.md", new byte[] {1}, null);
        OwnerFolderCopy.Result undated = OwnerFolderCopy.copy("grant", 1, "grant", 1, missing, dest, new AtomicBoolean());
        require(OwnerFolderCopy.NO_MTIME.equals(undated.ownerMessage));
        require(!dest.exists());
    }

    static void detachedPathAndBookmark() {
        OwnerFolderCopy.Result result = OwnerFolderCopy.refuseDetached("/hidden/page", "bookmark-secret");
        require(!result.authorityMatches);
        require(OwnerFolderCopy.DENIED.equals(result.ownerMessage));
        require(!result.ownerMessage.contains("/hidden/page"));
        require(!result.ownerMessage.contains("bookmark-secret"));
    }

    static void authorityMismatch() {
        MemoryFolder source = new MemoryFolder();
        source.dir("");
        File dest = new File(temp("authority"), "out");
        OwnerFolderCopy.Result token = OwnerFolderCopy.copy("grant", 1, "other", 1, source, dest, new AtomicBoolean());
        require(!token.authorityMatches);
        require(OwnerFolderCopy.DENIED.equals(token.ownerMessage));
        OwnerFolderCopy.Result generation = OwnerFolderCopy.copy("grant", 1, "grant", 2, source, dest, new AtomicBoolean());
        require(!generation.authorityMatches);
        require(OwnerFolderCopy.DENIED.equals(generation.ownerMessage));
        require(!dest.exists());
    }

    static void cancel() {
        MemoryFolder source = new MemoryFolder();
        source.dir("");
        source.file("a.md", new byte[] {1}, 1L);
        File dest = new File(temp("cancel"), "out");
        OwnerFolderCopy.Result result = OwnerFolderCopy.copy("grant", 1, "grant", 1, source, dest, new AtomicBoolean(true));
        require(result.authorityMatches);
        require(OwnerFolderCopy.CANCELLED.equals(result.ownerMessage));
        require(!dest.exists());
    }

    static void custodyAndSiblingStayWhenCopyFails() throws Exception {
        File held = new File(temp("custody"), "out");
        held.mkdirs();
        File custody = new File(held, "folder-custody.json");
        Files.write(custody.toPath(), "{\"adapter\":\"folder-copy\"}".getBytes("UTF-8"));
        File heldSibling = new File(held, "keep.txt");
        Files.write(heldSibling.toPath(), "stay".getBytes("UTF-8"));
        MemoryFolder blocked = new MemoryFolder();
        blocked.dir("");
        blocked.file("a.md", new byte[] {1}, 1L);
        OwnerFolderCopy.Result denied = OwnerFolderCopy.copy("grant", 1, "grant", 1, blocked, held, new AtomicBoolean());
        require(denied.authorityMatches);
        require(OwnerFolderCopy.DENIED.equals(denied.ownerMessage));
        require("{\"adapter\":\"folder-copy\"}".equals(bytes(custody)));
        require("stay".equals(bytes(heldSibling)));
        require(!new File(held, "a.md").exists());

        File later = new File(temp("partial"), "out");
        later.mkdirs();
        File keep = new File(later, "keep.txt");
        Files.write(keep.toPath(), "stay".getBytes("UTF-8"));
        MemoryFolder partial = new MemoryFolder();
        partial.dir("");
        partial.names.put("", new String[] {"new.md", "link"});
        partial.file("new.md", "created".getBytes("UTF-8"), 1L);
        partial.file("link", new byte[] {1}, 1L);
        partial.symlinks.add("link");
        OwnerFolderCopy.Result failed = OwnerFolderCopy.copy("grant", 1, "grant", 1, partial, later, new AtomicBoolean());
        require(OwnerFolderCopy.SPECIAL.equals(failed.ownerMessage));
        require("stay".equals(bytes(keep)));
        require(!new File(later, "new.md").exists());
        require(later.isDirectory());
    }

    static void prefixOccupiedEmptySymlinkAndLiveAuthority() throws Exception {
        File prefixDest = new File(temp("prefix"), "out");
        ShortFolder prefix = new ShortFolder(3);
        prefix.dir("");
        prefix.file("note.md", "abcdef".getBytes("UTF-8"), 1_700_000_000_000L);
        OwnerFolderCopy.Result grown = OwnerFolderCopy.copy("page", 1, "page", 1, prefix, prefixDest, new AtomicBoolean(false));
        require(!OwnerFolderCopy.READY.equals(grown.ownerMessage));
        require(OwnerFolderCopy.TRUNCATED.equals(grown.ownerMessage));
        require(!new File(prefixDest, "note.md").exists());

        File occupiedDest = new File(temp("occupied"), "out");
        occupiedDest.mkdirs();
        File occupiedNote = new File(occupiedDest, "note.md");
        Files.write(occupiedNote.toPath(), "priorOriginal".getBytes("UTF-8"));
        MemoryFolder occupied = new MemoryFolder();
        occupied.dir("");
        occupied.file("note.md", "new".getBytes("UTF-8"), 1_700_000_000_000L);
        OwnerFolderCopy.Result held = OwnerFolderCopy.copy("page", 1, "page", 1, occupied, occupiedDest, new AtomicBoolean(false));
        require(!OwnerFolderCopy.READY.equals(held.ownerMessage));
        require("priorOriginal".equals(bytes(occupiedNote)));

        File emptyDest = new File(temp("empty-dir"), "out");
        MemoryFolder empty = new MemoryFolder();
        empty.dir("");
        empty.dir("empty");
        OwnerFolderCopy.Result kept = OwnerFolderCopy.copy("page", 1, "page", 1, empty, emptyDest, new AtomicBoolean(false));
        require(kept.authorityMatches);
        require(OwnerFolderCopy.READY.equals(kept.ownerMessage));
        require(new File(emptyDest, "empty").isDirectory());

        File linkRoot = new File(temp("dest-link"), "out");
        linkRoot.mkdirs();
        File target = new File(linkRoot.getParentFile(), "target.txt");
        Files.write(target.toPath(), "priorOriginal".getBytes("UTF-8"));
        File link = new File(linkRoot, "note.md");
        Files.createSymbolicLink(link.toPath(), target.toPath());
        MemoryFolder linked = new MemoryFolder();
        linked.dir("");
        linked.file("note.md", "new".getBytes("UTF-8"), 1_700_000_000_000L);
        OwnerFolderCopy.Result skipped = OwnerFolderCopy.copy("page", 1, "page", 1, linked, linkRoot, new AtomicBoolean(false));
        require(!OwnerFolderCopy.READY.equals(skipped.ownerMessage));
        require("priorOriginal".equals(bytes(target)));
        require(Files.isSymbolicLink(link.toPath()));

        File liveDest = new File(temp("live-auth"), "out");
        liveDest.mkdirs();
        File sibling = new File(liveDest, "keep.txt");
        Files.write(sibling.toPath(), "stay".getBytes("UTF-8"));
        OwnerFolderCopy.LiveAuthority authority = new OwnerFolderCopy.LiveAuthority("page", 1, "page", 1);
        FlipOnOpen source = new FlipOnOpen(authority);
        source.dir("");
        source.file("note.md", "new".getBytes("UTF-8"), 1_700_000_000_000L);
        OwnerFolderCopy.Result moved = OwnerFolderCopy.copy(authority, source, liveDest, new AtomicBoolean(false));
        require(!moved.authorityMatches);
        require(!OwnerFolderCopy.READY.equals(moved.ownerMessage));
        require(!new File(liveDest, "note.md").exists());
        require("stay".equals(bytes(sibling)));
        require(liveDest.isDirectory());
    }

    static void sameGrant() {
        require(OwnerFolderCopy.sameGrant("content://folder/a", "content://folder/a"));
        require(!OwnerFolderCopy.sameGrant("content://folder/a", "content://folder/b"));
        require(!OwnerFolderCopy.sameGrant(null, null));
    }

    static String bytes(File file) throws Exception {
        return new String(Files.readAllBytes(file.toPath()), "UTF-8");
    }

    static void delete(File path) {
        File[] children = path.listFiles();
        if (children != null) {
            for (File child : children) {
                delete(child);
            }
        }
        path.delete();
    }

    static String temp(String name) {
        return System.getProperty("java.io.tmpdir") + File.separator + name + "-" + System.nanoTime();
    }

    static class MemoryFolder implements OwnerFolderCopy.FolderSource {
        final Map<String, byte[]> files = new HashMap<String, byte[]>();
        final Map<String, Long> mtimes = new HashMap<String, Long>();
        final Map<String, String[]> names = new HashMap<String, String[]>();
        final java.util.Set<String> directories = new java.util.HashSet<String>();
        final java.util.Set<String> symlinks = new java.util.HashSet<String>();

        void dir(String relative) {
            directories.add(relative);
        }

        void file(String relative, byte[] body, Long modified) {
            files.put(relative, body);
            mtimes.put(relative, modified);
        }

        public String[] children(String relative) {
            String[] listed = names.get(relative);
            if (listed != null) {
                return listed;
            }
            java.util.ArrayList<String> found = new java.util.ArrayList<String>();
            String prefix = relative.isEmpty() ? "" : relative + "/";
            java.util.Set<String> seen = new java.util.HashSet<String>();
            for (String path : files.keySet()) {
                if (!path.startsWith(prefix)) {
                    continue;
                }
                String rest = path.substring(prefix.length());
                int slash = rest.indexOf('/');
                String name = slash < 0 ? rest : rest.substring(0, slash);
                if (seen.add(name)) {
                    found.add(name);
                }
            }
            for (String path : directories) {
                if (path.isEmpty() || path.equals(relative) || !path.startsWith(prefix)) {
                    continue;
                }
                String rest = path.substring(prefix.length());
                int slash = rest.indexOf('/');
                if (slash >= 0) {
                    continue;
                }
                if (seen.add(rest)) {
                    found.add(rest);
                }
            }
            return found.toArray(new String[found.size()]);
        }

        public InputStream open(String relative) {
            return new ByteArrayInputStream(files.get(relative));
        }

        public long size(String relative) {
            return files.get(relative).length;
        }

        public boolean directory(String relative) {
            return directories.contains(relative);
        }

        public boolean symlink(String relative) {
            return symlinks.contains(relative);
        }

        public Long modifiedMillis(String relative) {
            return mtimes.get(relative);
        }
    }

    static final class ShortFolder extends MemoryFolder {
        final long declared;

        ShortFolder(long declared) {
            this.declared = declared;
        }

        public long size(String relative) {
            return declared;
        }
    }

    static final class FlipOnOpen extends MemoryFolder {
        final OwnerFolderCopy.LiveAuthority authority;

        FlipOnOpen(OwnerFolderCopy.LiveAuthority authority) {
            this.authority = authority;
        }

        public InputStream open(String relative) {
            authority.setLive("moved", 1);
            return super.open(relative);
        }
    }
}
