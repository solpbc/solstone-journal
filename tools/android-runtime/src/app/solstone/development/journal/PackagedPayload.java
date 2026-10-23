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
import java.nio.file.StandardCopyOption;
import java.nio.file.Path;
import java.util.Arrays;
import java.util.HashSet;
import java.util.Set;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;

/** Installs only the app-owned packaged payload before the worker is bound. */
final class PackagedPayload {
    interface Assets {
        String[] list(String name) throws IOException;
        InputStream open(String name) throws IOException;
    }

    static synchronized void unpack(Assets assets, String name, File destination) throws IOException {
        if (Files.isSymbolicLink(destination.toPath())) {
            throw new IOException("packaged payload destination is a symbolic link");
        }
        String[] children = assets.list(name);
        if (children == null) throw new IOException("packaged payload listing unavailable");
        if (children.length > 0) {
            if (Files.exists(destination.toPath(), LinkOption.NOFOLLOW_LINKS)
                    && !Files.isDirectory(destination.toPath(), LinkOption.NOFOLLOW_LINKS)) {
                Files.delete(destination.toPath());
            }
            if (!destination.mkdirs()
                    && !Files.isDirectory(destination.toPath(), LinkOption.NOFOLLOW_LINKS)) {
                throw new IOException("packaged payload directory unavailable");
            }
            for (String child : children) unpack(assets, name + "/" + child, new File(destination, child));
            Set<String> current = new HashSet<>(Arrays.asList(children));
            for (File child : childrenOf(destination)) {
                if (!current.contains(child.getName())) retire(child);
            }
        } else {
            if (Files.isRegularFile(destination.toPath(),LinkOption.NOFOLLOW_LINKS)) {
                try(InputStream packaged=assets.open(name);
                    InputStream installed=Files.newInputStream(destination.toPath(),LinkOption.NOFOLLOW_LINKS)) {
                    if (Arrays.equals(digest(packaged),digest(installed))) return;
                }
            }
            try (InputStream input = assets.open(name)) {
                if (Files.isDirectory(destination.toPath(), LinkOption.NOFOLLOW_LINKS)) retire(destination);
                Path stage=Files.createTempFile(destination.toPath().getParent(),".payload-",".stage");
                try {
                    try (OutputStream output = Files.newOutputStream(stage,StandardOpenOption.WRITE,LinkOption.NOFOLLOW_LINKS)) {
                        byte[] buffer = new byte[16384];
                        int count;
                        while ((count = input.read(buffer)) != -1) output.write(buffer, 0, count);
                    }
                    Files.move(stage,destination.toPath(),StandardCopyOption.ATOMIC_MOVE,StandardCopyOption.REPLACE_EXISTING);
                } finally { Files.deleteIfExists(stage); }
            }
        }
    }

    private static byte[] digest(InputStream input) throws IOException {
        MessageDigest digest;
        try { digest=MessageDigest.getInstance("SHA-256"); }
        catch(NoSuchAlgorithmException impossible) { throw new AssertionError(impossible); }
        byte[] buffer=new byte[16384]; int count;
        while((count=input.read(buffer)) != -1) digest.update(buffer,0,count);
        return digest.digest();
    }

    private static File[] childrenOf(File directory) throws IOException {
        File[] children = directory.listFiles();
        if (children == null) throw new IOException("installed payload listing unavailable");
        return children;
    }

    private static void retire(File entry) throws IOException {
        if (Files.isDirectory(entry.toPath(), LinkOption.NOFOLLOW_LINKS)) {
            for (File child : childrenOf(entry)) retire(child);
        }
        Files.delete(entry.toPath());
    }
}
