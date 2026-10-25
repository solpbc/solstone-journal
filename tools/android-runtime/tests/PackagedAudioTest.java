// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import java.io.*;
import java.nio.file.*;
import java.security.*;
import java.util.*;
import java.util.concurrent.*;
import java.util.concurrent.atomic.*;

public final class PackagedAudioTest {
    static void require(boolean value) { if (!value) throw new AssertionError(); }
    static final byte[] MODEL="a complete speech input".getBytes(java.nio.charset.StandardCharsets.UTF_8);
    static PackagedAudio.Pin pin(byte[] bytes) throws Exception {
        MessageDigest hash=MessageDigest.getInstance("SHA-256"); hash.update(bytes);
        return new PackagedAudio.Pin("model.gguf",bytes.length,PackagedAudio.digest(hash));
    }
    static PackagedPayload.Assets assets(byte[] bytes) {
        return new PackagedPayload.Assets() {
            public String[] list(String path) { return new String[0]; }
            public InputStream open(String path) { return new ByteArrayInputStream(bytes); }
        };
    }
    static void prepare(Path root,PackagedPayload.Assets assets,PackagedAudio.Pin pin,Runnable ready) throws Exception {
        PackagedAudio.prepare(assets,Arrays.asList(pin),Collections.emptyList(),root.resolve("models").toFile(),
            root.resolve("staging").toFile(),()->true,ready);
    }
    static void remove(Path path) throws IOException {
        try(java.util.stream.Stream<Path> walk=Files.walk(path)) {
            for(Path member:(Iterable<Path>)walk.sorted(Comparator.reverseOrder())::iterator) Files.delete(member);
        }
    }
    static void refusal(RunnableThrowing action) throws Exception {
        try { action.run();throw new AssertionError("unsafe preparation accepted"); }
        catch(IOException expected) {}
    }
    interface RunnableThrowing { void run() throws Exception; }
    static void additiveAndRecovery(Path root) throws Exception {
        Path models=Files.createDirectory(root.resolve("models"));
        Path inherited=models.resolve("kept-original.onnx"); Files.write(inherited,MODEL);
        AtomicInteger binds=new AtomicInteger();
        prepare(root,assets(MODEL),pin(MODEL),binds::incrementAndGet);
        require(Arrays.equals(Files.readAllBytes(models.resolve("model.gguf")),MODEL));
        require(Arrays.equals(Files.readAllBytes(inherited),MODEL));
        Path marker=root.resolve("staging/owned-v1");
        Files.write(marker.resolve("payload.stage"),new byte[]{7,8,9});
        Files.write(marker.resolve("unrelated.keep"),MODEL);
        prepare(root,assets(MODEL),pin(MODEL),binds::incrementAndGet);
        require(!Files.exists(marker.resolve("payload.stage")));
        require(Arrays.equals(Files.readAllBytes(marker.resolve("unrelated.keep")),MODEL));
        byte[] conflict=MODEL.clone(); conflict[0]^=1; Files.write(models.resolve("model.gguf"),conflict);
        refusal(()->prepare(root,assets(MODEL),pin(MODEL),binds::incrementAndGet));
        require(Arrays.equals(Files.readAllBytes(models.resolve("model.gguf")),conflict));
        require(binds.get()==2);
    }
    static void noticeUpgrade(Path root) throws Exception {
        Path notices=Files.createDirectories(root.resolve("models/notices"));
        byte[] previous="previous aggregate notice".getBytes("UTF-8"), next="current aggregate notice".getBytes("UTF-8");
        Path inherited=notices.resolve("NOTICE.md");Files.write(inherited,previous);
        MessageDigest hash=PackagedAudio.hash();hash.update(next);String digest=PackagedAudio.digest(hash);
        PackagedAudio.Pin updated=new PackagedAudio.Pin("NOTICE-"+digest+".md",next.length,digest);
        PackagedPayload.Assets source=new PackagedPayload.Assets() {
            public String[] list(String path) { return new String[0]; }
            public InputStream open(String path) { return new ByteArrayInputStream(path.startsWith("audio-models/notices/") ? next : MODEL); }
        };
        AtomicInteger ready=new AtomicInteger();
        for(int i=0;i<2;i++) PackagedAudio.prepare(source,Arrays.asList(pin(MODEL)),Arrays.asList(updated),
            root.resolve("models").toFile(),root.resolve("staging").toFile(),()->true,ready::incrementAndGet);
        require(ready.get()==2);
        require(Arrays.equals(Files.readAllBytes(inherited),previous));
        require(Arrays.equals(Files.readAllBytes(notices.resolve(updated.name)),next));
        require(Arrays.equals(Files.readAllBytes(root.resolve("models/model.gguf")),MODEL));
        PackagedAudio.Pin colliding=new PackagedAudio.Pin("NOTICE.md",next.length,digest);
        refusal(()->PackagedAudio.prepare(source,Arrays.asList(pin(MODEL)),Arrays.asList(colliding),
            root.resolve("models").toFile(),root.resolve("staging").toFile(),()->true,ready::incrementAndGet));
        require(ready.get()==2 && Arrays.equals(Files.readAllBytes(inherited),previous));
    }
    static void failedCopyAndRetry(Path root) throws Exception {
        PackagedPayload.Assets failing=new PackagedPayload.Assets() {
            public String[] list(String path) { return new String[0]; }
            public InputStream open(String path) { return new InputStream() {
                int offset;
                public int read() throws IOException { if(offset++<3)return 1;throw new IOException("interrupted input"); }
            }; }
        };
        AtomicInteger binds=new AtomicInteger();
        refusal(()->prepare(root,failing,pin(MODEL),binds::incrementAndGet));
        require(!Files.exists(root.resolve("models/model.gguf")));
        require(!Files.exists(root.resolve("staging/owned-v1/payload.stage")));
        require(binds.get()==0);
        prepare(root,assets(MODEL),pin(MODEL),binds::incrementAndGet);
        require(binds.get()==1);
    }
    static void bootstrapAndLinks(Path parent) throws Exception {
        Path empty=Files.createDirectory(parent.resolve("empty")); Files.createDirectory(empty.resolve("staging"));
        prepare(empty,assets(MODEL),pin(MODEL),()->{});
        Path unowned=Files.createDirectory(parent.resolve("unowned"));
        Path staging=Files.createDirectory(unowned.resolve("staging")); Files.write(staging.resolve("payload.stage"),MODEL);
        refusal(()->prepare(unowned,assets(MODEL),pin(MODEL),()->{}));
        require(Arrays.equals(Files.readAllBytes(staging.resolve("payload.stage")),MODEL));
        for(String part:Arrays.asList("models","staging","staging/owned-v1","staging/owned-v1/payload.stage","models/model.gguf")) {
            Path root=Files.createTempDirectory(parent,"link-"); Path outside=Files.createTempDirectory(parent,"outside-");
            Path target=outside.resolve("target"); Files.write(target,MODEL);
            Path link=root.resolve(part); Files.createDirectories(link.getParent());
            Files.createSymbolicLink(link,part.equals("models") || part.equals("staging") || part.equals("staging/owned-v1") ? outside : target);
            refusal(()->prepare(root,assets(MODEL),pin(MODEL),()->{}));
            require(Files.isSymbolicLink(link));require(Arrays.equals(Files.readAllBytes(target),MODEL));
        }
    }
    static void recreation(Path root) throws Exception {
        CountDownLatch reading=new CountDownLatch(1),release=new CountDownLatch(1),secondEntered=new CountDownLatch(1);
        AtomicBoolean current=new AtomicBoolean(true); AtomicInteger oldBinds=new AtomicInteger(),newBinds=new AtomicInteger();
        AtomicReference<Throwable> failure=new AtomicReference<>();
        PackagedPayload.Assets held=new PackagedPayload.Assets() {
            public String[] list(String path) { return new String[0]; }
            public InputStream open(String path) { return new ByteArrayInputStream(MODEL) {
                public synchronized int read(byte[] bytes,int off,int len) {
                    reading.countDown();
                    try { release.await(); } catch(InterruptedException error) { Thread.currentThread().interrupt(); }
                    return super.read(bytes,off,len);
                }
            }; }
        };
        Thread old=new Thread(()->{
            try { PackagedAudio.prepare(held,Arrays.asList(uncheckedPin()),Collections.emptyList(),root.resolve("models").toFile(),
                root.resolve("staging").toFile(),current::get,oldBinds::incrementAndGet); }
            catch(IOException expected) {} catch(Throwable error) { failure.set(error); }
        });
        Thread replacement=new Thread(()->{
            secondEntered.countDown();
            try { prepare(root,assets(MODEL),pin(MODEL),newBinds::incrementAndGet); }
            catch(Throwable error) { failure.set(error); }
        });
        old.start(); require(reading.await(5,TimeUnit.SECONDS)); replacement.start();require(secondEntered.await(5,TimeUnit.SECONDS));
        long deadline=System.nanoTime()+TimeUnit.SECONDS.toNanos(5);
        while(replacement.getState()!=Thread.State.BLOCKED && System.nanoTime()<deadline) Thread.yield();
        require(replacement.getState()==Thread.State.BLOCKED);require(newBinds.get()==0);
        current.set(false); old.interrupt(); release.countDown(); old.join(5000); replacement.join(5000);
        require(!old.isAlive() && !replacement.isAlive()); if(failure.get()!=null) throw new AssertionError(failure.get());
        require(oldBinds.get()==0 && newBinds.get()==1);
        require(Arrays.equals(Files.readAllBytes(root.resolve("models/model.gguf")),MODEL));
    }
    static PackagedAudio.Pin uncheckedPin() {
        try { return pin(MODEL); } catch(Exception impossible) { throw new AssertionError(impossible); }
    }
    static void enospc(Path parent) throws Exception {
        Path root=Files.createTempDirectory(parent,"audio-full-");
        byte[] large=new byte[2*1024*1024];
        AtomicInteger binds=new AtomicInteger();
        try {
            Files.write(root.resolve("canary"),MODEL);
            try { prepare(root,assets(large),pin(large),binds::incrementAndGet);throw new AssertionError("bounded filesystem did not fill"); }
            catch(IOException expected) {
                require(expected.toString().contains("No space left on device"));
                System.out.println("actual bounded-filesystem ENOSPC: "+expected.getClass().getSimpleName());
            }
            require(binds.get()==0 && !Files.exists(root.resolve("models/model.gguf")));
            require(!Files.exists(root.resolve("staging/owned-v1/payload.stage")));
            require(Arrays.equals(Files.readAllBytes(root.resolve("canary")),MODEL));
        } finally { remove(root); }
    }
    public static void main(String[] args) throws Exception {
        if(args.length==2 && args[0].equals("--enospc")) { enospc(Paths.get(args[1]));return; }
        Path root=Files.createTempDirectory("packaged-audio-");
        try {
            additiveAndRecovery(Files.createDirectory(root.resolve("additive")));
            noticeUpgrade(Files.createDirectory(root.resolve("notice-upgrade")));
            failedCopyAndRetry(Files.createDirectory(root.resolve("retry")));
            bootstrapAndLinks(Files.createDirectory(root.resolve("boundaries")));
            recreation(Files.createDirectory(root.resolve("recreation")));
            System.out.println("packaged audio: preservation, interrupted retry, abandoned stage, links and recreation passed");
        } finally { remove(root); }
    }
}
