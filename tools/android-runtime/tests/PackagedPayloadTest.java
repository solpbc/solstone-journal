// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import java.io.ByteArrayInputStream;
import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.LinkOption;
import java.nio.file.Path;
import java.util.Arrays;
import java.util.Comparator;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.TreeMap;
import java.util.TreeSet;
import java.util.stream.Stream;

public final class PackagedPayloadTest {
    private static final class Assets implements PackagedPayload.Assets {
        final Map<String, String> files = new LinkedHashMap<>();
        String failRead;
        String failList;
        public String[] list(String name) throws IOException {
            if (name.equals(failList)) throw new IOException("fixture listing failure");
            TreeSet<String> names = new TreeSet<>();
            for (String path : files.keySet()) {
                if (path.startsWith(name + "/")) names.add(path.substring(name.length() + 1).split("/")[0]);
            }
            return names.toArray(new String[0]);
        }
        public InputStream open(String name) throws IOException {
            if (!files.containsKey(name)) throw new IOException("fixture asset unavailable");
            final byte[] bytes = files.get(name).getBytes(StandardCharsets.UTF_8);
            if (!name.equals(failRead)) return new ByteArrayInputStream(bytes);
            return new InputStream() {
                boolean first = true;
                public int read() throws IOException { throw new IOException("fixture partial read failure"); }
                public int read(byte[] buffer, int offset, int length) throws IOException {
                    if (!first) throw new IOException("fixture partial read failure");
                    first = false;
                    int count = Math.min(length, Math.min(2, bytes.length));
                    System.arraycopy(bytes, 0, buffer, offset, count);
                    return count;
                }
            };
        }
    }

    private static Assets assets() {
        Assets assets = new Assets();
        assets.files.put("payload/solstone/talent/current.md", "current packaged talent");
        assets.files.put("payload/to-file", "new file");
        assets.files.put("payload/to-directory/child", "new child");
        return assets;
    }

    private static void write(Path path, String content) throws IOException {
        Files.createDirectories(path.getParent());
        Files.write(path, content.getBytes(StandardCharsets.UTF_8));
    }

    private static Map<String, String> census(Path root) throws IOException {
        Map<String, String> result = new TreeMap<>();
        try (Stream<Path> paths = Files.walk(root)) {
            for (Path path : (Iterable<Path>) paths::iterator) {
                if (path.equals(root)) continue;
                String kind = Files.isSymbolicLink(path) ? "symlink"
                        : Files.isDirectory(path, LinkOption.NOFOLLOW_LINKS) ? "directory"
                        : Files.isRegularFile(path, LinkOption.NOFOLLOW_LINKS) ? "file:" + new String(Files.readAllBytes(path), StandardCharsets.UTF_8)
                        : "special";
                result.put(root.relativize(path).toString(), kind);
            }
        }
        return result;
    }

    private static void exact(Path root, Assets assets) throws IOException {
        Map<String, String> expected = new TreeMap<>();
        for (Map.Entry<String, String> file : assets.files.entrySet()) {
            Path name = new File(file.getKey().substring("payload/".length())).toPath();
            expected.put(name.toString(), "file:" + file.getValue());
            for (Path parent = name.getParent(); parent != null; parent = parent.getParent()) expected.put(parent.toString(), "directory");
        }
        if (!expected.equals(census(root))) throw new AssertionError("installed payload must match all packaged entries and bytes");
    }

    private static void expectFailure(PackagedPayload.Assets assets, Path root) throws IOException {
        try {
            PackagedPayload.unpack(assets, "payload", root.toFile());
            throw new AssertionError("unsafe or failed extraction must refuse");
        } catch (IOException expected) { }
    }

    private static void upgrade(Path base) throws IOException {
        Path root = base.resolve("payload");
        write(root.resolve("solstone/talent/current.md"), "old version");
        write(root.resolve("solstone/talent/facet_newsletter.md"), "retired talent");
        write(root.resolve("obsolete/deep/file"), "old asset");
        write(root.resolve("to-file/old"), "old directory");
        write(root.resolve("to-directory"), "old file");
        Path canary = base.resolve("unrelated/canary");
        write(canary, "preserve unrelated data");
        Path outside = base.resolve("outside");
        write(outside.resolve("canary"), "preserve symlink target");
        Files.createSymbolicLink(root.resolve("stale-link"), outside);
        Assets assets = assets();
        PackagedPayload.unpack(assets, "payload", root.toFile());
        exact(root, assets);
        if (!new String(Files.readAllBytes(canary), StandardCharsets.UTF_8).equals("preserve unrelated data")
                || !new String(Files.readAllBytes(outside.resolve("canary")), StandardCharsets.UTF_8).equals("preserve symlink target")) {
            throw new AssertionError("payload upgrade changed unrelated data");
        }
    }

    private static void retireTalent(Path base) throws IOException {
        Path root = base.resolve("payload");
        write(root.resolve("solstone/talent/current.md"), "old version");
        write(root.resolve("solstone/talent/facet_newsletter.md"), "retired talent");
        Assets assets = new Assets();
        assets.files.put("payload/solstone/talent/current.md", "current packaged talent");
        PackagedPayload.unpack(assets, "payload", root.toFile());
        exact(root, assets);
    }

    private static void retry(Path base, boolean readFailure) throws IOException {
        Path root = base.resolve("payload");
        write(root.resolve("retired.md"), "retired");
        Path canary = base.resolve("canary");
        write(canary, "preserved");
        Assets assets = assets();
        if (readFailure) assets.failRead = "payload/solstone/talent/current.md";
        else assets.failList = "payload/to-file";
        expectFailure(assets, root);
        if (readFailure && Files.exists(root.resolve("solstone/talent/current.md"))) throw new AssertionError("failed copy published partial bytes");
        assets.failRead = null;
        assets.failList = null;
        PackagedPayload.unpack(assets, "payload", root.toFile());
        exact(root, assets);
        if (!Arrays.equals(Files.readAllBytes(canary), "preserved".getBytes(StandardCharsets.UTF_8))) throw new AssertionError("retry changed sibling");
    }

    private static void destinationSymlinks(Path base) throws IOException {
        Path root = base.resolve("payload");
        Path target = base.resolve("outside");
        write(target, "unchanged");
        Files.createDirectories(root.resolve("solstone/talent"));
        Files.createSymbolicLink(root.resolve("solstone/talent/current.md"), target);
        expectFailure(assets(), root);
        if (!new String(Files.readAllBytes(target), StandardCharsets.UTF_8).equals("unchanged")) throw new AssertionError("copied through destination link");
        Path rootLink = base.resolve("root-link");
        Files.createSymbolicLink(rootLink, root);
        expectFailure(assets(), rootLink);
    }

    public static void main(String[] args) throws Exception {
        Path base = Files.createTempDirectory("packaged-payload-test-");
        try {
            retireTalent(base.resolve("retired-talent"));
            upgrade(base.resolve("upgrade"));
            retry(base.resolve("read-retry"), true);
            retry(base.resolve("list-retry"), false);
            destinationSymlinks(base.resolve("symlinks"));
            reopenDoesNotTruncateActivePayload(base.resolve("reopen"));
            System.out.println("packaged payload upgrade, retry and no-follow checks passed");
        } finally {
            try (Stream<Path> paths = Files.walk(base)) {
                for (Path path : (Iterable<Path>) paths.sorted(Comparator.reverseOrder())::iterator) Files.delete(path);
            }
        }
    }
    private static void reopenDoesNotTruncateActivePayload(Path root) throws Exception {
        Files.createDirectories(root);
        Path target=root.resolve("talent.md"); byte[] original="complete installed payload".getBytes(StandardCharsets.UTF_8);
        Files.write(target,original);
        java.util.concurrent.CountDownLatch reading=new java.util.concurrent.CountDownLatch(1),release=new java.util.concurrent.CountDownLatch(1);
        java.util.concurrent.atomic.AtomicReference<Throwable> failure=new java.util.concurrent.atomic.AtomicReference<>();
        PackagedPayload.Assets assets=new PackagedPayload.Assets() {
            public String[] list(String name) { return new String[0]; }
            public InputStream open(String name) { return new ByteArrayInputStream(original) {
                public synchronized int read(byte[] bytes,int offset,int length) {
                    reading.countDown();
                    try { release.await(); } catch(InterruptedException error) { Thread.currentThread().interrupt(); }
                    return super.read(bytes,offset,length);
                }
            }; }
        };
        Thread reopen=new Thread(()->{
            try { PackagedPayload.unpack(assets,"talent.md",target.toFile()); }
            catch(Throwable error) { failure.set(error); }
        });
        reopen.start();
        try {
            if (!reading.await(5,java.util.concurrent.TimeUnit.SECONDS)) throw new AssertionError("asset read never started");
            if (!Arrays.equals(Files.readAllBytes(target),original)) throw new AssertionError("reopening truncated a payload still in use");
        } finally { release.countDown();reopen.join(5000); }
        if (reopen.isAlive() || failure.get()!=null) throw new AssertionError("reopen did not complete",failure.get());
    }
}
