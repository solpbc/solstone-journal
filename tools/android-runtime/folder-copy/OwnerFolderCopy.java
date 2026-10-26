// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.file.Files;
import java.nio.file.LinkOption;
import java.nio.file.StandardOpenOption;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.function.BooleanSupplier;

/** Copies a picked folder without taking a page path or an Android permission. */
final class OwnerFolderCopy {
    static final int BUFFER_BYTES = 65536;
    static final int MAX_DESCRIPTORS = 8;

    static final String READY = "Folder ready to preview.";
    static final String CANCELLED = "Folder selection cancelled.";
    static final String DENIED = "The folder could not be read.";
    static final String UNAVAILABLE = "The selected folder is no longer available.";
    static final String STORAGE_FULL = "Copy stopped because storage is full.";
    static final String TRUNCATED = "Copy stopped because the folder changed during the copy.";
    static final String NO_MTIME = "A file in this folder has no modified time.";
    static final String SPECIAL = "import sources must contain only regular files and folders";
    static final String ESCAPE = "That entry is outside the selected folder.";
    static final String DUPLICATE = "That folder contains two entries with the same name.";

    static final class Result {
        final boolean authorityMatches;
        final String ownerMessage;

        Result(boolean authorityMatches, String ownerMessage) {
            this.authorityMatches = authorityMatches;
            this.ownerMessage = ownerMessage;
        }
    }

    /**
     * Page authority re-read before each file and before ready.
     * The string overload stores the caller's tokens here.
     */
    static final class LiveAuthority {
        final String requestedToken;
        final int requestedGeneration;
        private String liveToken;
        private int liveGeneration;
        private final BooleanSupplier continues;

        LiveAuthority(String requestedToken, int requestedGeneration, String liveToken, int liveGeneration) {
            this(requestedToken, requestedGeneration, liveToken, liveGeneration, new BooleanSupplier() {
                public boolean getAsBoolean() { return true; }
            });
        }

        LiveAuthority(String requestedToken, int requestedGeneration, String liveToken, int liveGeneration,
                BooleanSupplier continues) {
            this.continues = continues;
            this.requestedToken = requestedToken;
            this.requestedGeneration = requestedGeneration;
            this.liveToken = liveToken;
            this.liveGeneration = liveGeneration;
        }

        synchronized void setLive(String token, int generation) {
            liveToken = token;
            liveGeneration = generation;
        }

        synchronized boolean matches() {
            return continues.getAsBoolean() && requestedToken != null && requestedToken.equals(liveToken) && requestedGeneration == liveGeneration;
        }
    }

    /** Listing and file facts supplied by the picker. A missing modified time is null. */
    interface FolderSource {
        String[] children(String relative) throws IOException;

        InputStream open(String relative) throws IOException;

        /** Byte size, or -1 when the provider does not publish it. */
        long size(String relative) throws IOException;

        boolean directory(String relative) throws IOException;

        boolean symlink(String relative) throws IOException;

        Long modifiedMillis(String relative) throws IOException;
    }

    private static final class Budget {
        private int live;
        private final int limit;

        Budget(int limit) {
            this.limit = limit;
        }

        void acquire() {
            if (live >= limit) {
                throw new Stop(DENIED);
            }
            live++;
        }

        void release() {
            if (live > 0) {
                live--;
            }
        }
    }

    private static class Stop extends RuntimeException {
        final String message;

        Stop(String message) {
            this.message = message;
        }
    }

    private static final class AuthorityChanged extends Stop {
        AuthorityChanged() {
            super(DENIED);
        }
    }

    private OwnerFolderCopy() {}

    static Result copy(
            String requestedToken,
            int requestedGeneration,
            String liveToken,
            int liveGeneration,
            FolderSource source,
            File destination,
            AtomicBoolean cancel) {
        return copy(
                new LiveAuthority(requestedToken, requestedGeneration, liveToken, liveGeneration),
                source,
                destination,
                cancel);
    }

    static Result copy(LiveAuthority authority, FolderSource source, File destination, AtomicBoolean cancel) {
        if (!authority.matches()) {
            return new Result(false, DENIED);
        }
        if (new File(destination, "folder-custody.json").isFile()) {
            return new Result(true, DENIED);
        }
        if (cancel != null && cancel.get()) {
            return new Result(true, CANCELLED);
        }
        if (!safeDirectoryChain(destination)) {
            return new Result(true, DENIED);
        }
        boolean createdRoot = !destination.exists();
        if (!destination.mkdirs() && !destination.isDirectory()) {
            return new Result(true, DENIED);
        }
        List<File> created = new ArrayList<File>();
        if (createdRoot) {
            created.add(destination);
        }
        try {
            copyTree(authority, source, "", destination, new byte[BUFFER_BYTES], new Budget(MAX_DESCRIPTORS), cancel, new HashSet<String>(), created);
            if (cancel != null && cancel.get()) {
                deleteCreated(created);
                return new Result(true, CANCELLED);
            }
            if (!authority.matches()) {
                deleteCreated(created);
                return new Result(false, DENIED);
            }
            return new Result(true, READY);
        } catch (AuthorityChanged changed) {
            deleteCreated(created);
            return new Result(false, DENIED);
        } catch (Stop stop) {
            deleteCreated(created);
            return new Result(true, stop.message);
        } catch (IOException exception) {
            deleteCreated(created);
            return new Result(true, DENIED);
        }
    }

    /** A detached path or bookmark is not a folder page. The denial does not echo either value. */
    static Result refuseDetached(String path, String bookmark) {
        if (path == null && bookmark == null) {
            return new Result(false, DENIED);
        }
        return new Result(false, DENIED);
    }

    static boolean sameGrant(String picker, String stored) {
        return picker != null && picker.equals(stored);
    }

    private static void copyTree(
            LiveAuthority authority,
            FolderSource source,
            String relative,
            File destination,
            byte[] buffer,
            Budget budget,
            AtomicBoolean cancel,
            HashSet<String> seen,
            List<File> created) throws IOException {
        ensureCurrent(authority, cancel);
        if (source.symlink(relative)) {
            throw new Stop(SPECIAL);
        }
        if (source.directory(relative)) {
            if (!relative.isEmpty()) {
                createDirectory(source, relative, destination, created);
            }
            budget.acquire();
            String[] names;
            try {
                names = source.children(relative);
            } finally {
                budget.release();
            }
            if (names == null) {
                throw new Stop(DENIED);
            }
            for (String name : names) {
                if (!oneComponent(name)) {
                    throw new Stop(ESCAPE);
                }
                String child = relative.isEmpty() ? name : relative + "/" + name;
                if (!seen.add(child)) {
                    throw new Stop(DUPLICATE);
                }
                copyTree(authority, source, child, destination, buffer, budget, cancel, seen, created);
            }
            if (!relative.isEmpty()) {
                Long modified = source.modifiedMillis(relative);
                File copiedDirectory = new File(destination, relative);
                if (created.contains(copiedDirectory) && modified != null && !copiedDirectory.setLastModified(modified)) {
                    throw new Stop(DENIED);
                }
            }
            return;
        }
        ensureCurrent(authority, cancel);
        Long modified = source.modifiedMillis(relative);
        if (modified == null) {
            throw new Stop(NO_MTIME);
        }
        long expected = source.size(relative);
        File outputFile = new File(destination, relative);
        if (!safeDirectoryChain(outputFile.getParentFile())) {
            throw new Stop(DENIED);
        }
        budget.acquire();
        try (InputStream stream = source.open(relative)) {
            ensureCurrent(authority, cancel);
            budget.acquire();
            try {
                OutputStream exclusive = Files.newOutputStream(
                        outputFile.toPath(), StandardOpenOption.CREATE_NEW, StandardOpenOption.WRITE);
                created.add(outputFile);
                try (OutputStream output = exclusive) {
                    long copied = 0;
                    while (true) {
                        ensureCurrent(authority, cancel);
                        int read = stream.read(buffer, 0, buffer.length);
                        ensureCurrent(authority, cancel);
                        if (read < 0) { break; }
                        if (read == 0) { throw new Stop(DENIED); }
                        if (expected >= 0 && copied + read > expected) { throw new Stop(TRUNCATED); }
                        output.write(buffer, 0, read);
                        copied += read;
                    }
                    long finalSize = source.size(relative);
                    if ((expected >= 0 && copied != expected)
                            || (finalSize >= 0 && copied != finalSize)
                            || !modified.equals(source.modifiedMillis(relative))) {
                        throw new Stop(TRUNCATED);
                    }
                }
            } finally {
                budget.release();
            }
        } finally {
            budget.release();
        }
        if (!outputFile.setLastModified(modified)) {
            throw new Stop(DENIED);
        }
    }

    private static void createDirectory(
            FolderSource source, String relative, File destination, List<File> created) throws IOException {
        File dir = new File(destination, relative);
        if (!safeDirectoryChain(dir)) { throw new Stop(DENIED); }
        boolean createdDir = false;
        if (Files.exists(dir.toPath(), LinkOption.NOFOLLOW_LINKS)) {
            if (Files.isSymbolicLink(dir.toPath()) || !dir.isDirectory()) {
                throw new Stop(DENIED);
            }
        } else {
            File parent = dir.getParentFile();
            if (parent != null) {
                noteMissing(destination, parent, created);
            }
            if (!dir.mkdirs() && !dir.isDirectory()) {
                throw new Stop(DENIED);
            }
            created.add(dir);
            createdDir = true;
        }
        Long modified = source.modifiedMillis(relative);
        if (createdDir && modified != null && !dir.setLastModified(modified)) {
            throw new Stop(DENIED);
        }
    }

    private static void ensureCurrent(LiveAuthority authority, AtomicBoolean cancel) {
        if (cancel != null && cancel.get()) {
            throw new Stop(CANCELLED);
        }
        if (!authority.matches()) {
            throw new AuthorityChanged();
        }
    }

    private static boolean safeDirectoryChain(File directory) {
        for (File cursor = directory; cursor != null; cursor = cursor.getParentFile()) {
            if (Files.isSymbolicLink(cursor.toPath())) { return false; }
            if (Files.exists(cursor.toPath(), LinkOption.NOFOLLOW_LINKS) && !cursor.isDirectory()) {
                return false;
            }
        }
        return true;
    }

    private static boolean oneComponent(String name) {
        return name != null
                && !name.isEmpty()
                && !name.equals(".")
                && !name.equals("..")
                && name.indexOf('/') < 0
                && name.indexOf('\\') < 0;
    }

    private static void noteMissing(File root, File path, List<File> created) {
        ArrayList<File> missing = new ArrayList<File>();
        File cursor = path;
        while (cursor != null && !cursor.equals(root) && !cursor.exists()) {
            missing.add(cursor);
            cursor = cursor.getParentFile();
        }
        for (int index = missing.size() - 1; index >= 0; index--) {
            created.add(missing.get(index));
        }
    }

    private static void deleteCreated(List<File> created) {
        for (int index = created.size() - 1; index >= 0; index--) {
            created.get(index).delete();
        }
    }
}
