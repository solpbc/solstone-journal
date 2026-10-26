// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.FileAlreadyExistsException;
import java.nio.file.Files;
import java.nio.file.LinkOption;
import java.nio.file.StandardOpenOption;
import java.util.UUID;

/**
 * Exclusive folder-copy ownership for the activity process. A restarted
 * counter never reuses a directory, receipt, or claim that already exists.
 * release deletes only the directory and receipt this claim created.
 * The abandoned predicate is the host stand-in for the serving reclaim.
 * A numeric copy with a claim and no receipt is the partial left when the
 * process dies after the claim is created and before a receipt exists.
 */
final class OwnerBridgeFiles {
    static final class Claim {
        final long id;
        final String token;
        boolean receiptCreated;
        Claim(long id, String token) { this.id = id; this.token = token; }
    }

    static File directory(File cache, long id) {
        return new File(new File(cache, "owner-bridge"), Long.toString(id));
    }

    static Claim allocate(File cache) throws IOException {
        File root = new File(cache, "owner-bridge");
        if (Files.isSymbolicLink(root.toPath())) throw new IOException("couldn't be read");
        if (!root.isDirectory() && !root.mkdirs()) throw new IOException("couldn't be read");
        for (long id = 1; id > 0; id++) {
            File dir = new File(root, Long.toString(id));
            File receipt = new File(root, id + ".receipt");
            File claim = new File(root, id + ".claim");
            if (occupied(dir) || occupied(receipt) || occupied(claim)) continue;
            String token = UUID.randomUUID().toString();
            boolean madeClaim = false;
            try {
                Files.write(claim.toPath(), token.getBytes(StandardCharsets.UTF_8), StandardOpenOption.CREATE_NEW);
                madeClaim = true;
                Files.createDirectory(dir.toPath());
                return new Claim(id, token);
            } catch (FileAlreadyExistsException ignored) {
                if (madeClaim && tokenMatches(claim, token)) claim.delete();
                continue;
            } catch (IOException error) {
                if (madeClaim && tokenMatches(claim, token)) claim.delete();
                throw error;
            }
        }
        throw new IOException("couldn't be read");
    }

    static void commit(File cache, Claim claim) throws IOException {
        File claimFile = claimFile(cache, claim.id);
        if (!tokenMatches(claimFile, claim.token)) throw new IOException("couldn't be read");
        File receipt = new File(claimFile.getParentFile(), claim.id + ".receipt");
        byte[] body = ("{\"id\":" + claim.id + "}\n").getBytes(StandardCharsets.UTF_8);
        Files.write(receipt.toPath(), body, StandardOpenOption.CREATE_NEW);
        claim.receiptCreated = true;
    }

    static void release(File cache, Claim claim) {
        if (cache == null || claim == null) return;
        File claimFile = claimFile(cache, claim.id);
        if (!tokenMatches(claimFile, claim.token)) return;
        File dir = directory(cache, claim.id);
        try {
            if (Files.isSymbolicLink(dir.toPath())) return;
            if (dir.isDirectory()) deleteOwnedTree(dir);
            if (claim.receiptCreated) {
                File receipt = new File(claimFile.getParentFile(), claim.id + ".receipt");
                if (!Files.isSymbolicLink(receipt.toPath()) && Files.isRegularFile(receipt.toPath(), LinkOption.NOFOLLOW_LINKS)) {
                    receipt.delete();
                }
            }
        } catch (IOException ignored) {
            return;
        }
        if (tokenMatches(claimFile, claim.token)) claimFile.delete();
    }

    static void disarm(File cache, Claim claim) {
        if (cache == null || claim == null) return;
        File claimFile = claimFile(cache, claim.id);
        if (tokenMatches(claimFile, claim.token)) claimFile.delete();
    }

    private static File claimFile(File cache, long id) {
        return new File(new File(cache, "owner-bridge"), id + ".claim");
    }

    private static boolean occupied(File file) {
        return Files.exists(file.toPath(), LinkOption.NOFOLLOW_LINKS);
    }

    private static boolean tokenMatches(File claim, String token) {
        try {
            if (token == null || token.length() == 0 || token.length() > 128) return false;
            if (!Files.isRegularFile(claim.toPath(), LinkOption.NOFOLLOW_LINKS)) return false;
            byte[] bytes = Files.readAllBytes(claim.toPath());
            if (bytes.length > 128) return false;
            return token.equals(new String(bytes, StandardCharsets.UTF_8));
        } catch (IOException error) {
            return false;
        }
    }

    private static void deleteOwnedTree(File file) throws IOException {
        if (Files.isSymbolicLink(file.toPath())) return;
        File[] children = file.listFiles();
        if (children != null) {
            for (File child : children) {
                if (Files.isSymbolicLink(child.toPath())) Files.deleteIfExists(child.toPath());
                else deleteOwnedTree(child);
            }
        }
        Files.deleteIfExists(file.toPath());
    }

    static boolean abandoned(String name, String path, boolean receipt, boolean claim, String[] admitted, long[] live) {
        if (!receipt && !claim) return false;
        Long id = null;
        if (name != null) {
            try {
                id = Long.parseLong(name);
            } catch (NumberFormatException ignored) { /* A non-numeric child can still be admitted. */ }
        }
        if (!receipt && id == null) return false;
        if (id != null && live != null) {
            for (int index = 0; index < live.length; index++) {
                if (live[index] == id.longValue()) return false;
            }
        }
        if (admitted != null && path != null) {
            String prefix = path.endsWith("/") ? path : path + "/";
            for (int index = 0; index < admitted.length; index++) {
                String kept = admitted[index];
                if (kept == null) continue;
                if (kept.equals(path) || kept.startsWith(prefix)) return false;
            }
        }
        return true;
    }
}
