// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import java.io.*;
import java.nio.channels.*;
import java.nio.file.*;
import java.security.*;
import java.util.*;
import java.util.function.BooleanSupplier;
import java.util.function.LongConsumer;

/** Additive installation only: existing speech inputs are never replaced. */
final class PackagedAudio {
    static final class Pin {
        final String name, sha256;
        final long bytes;
        Pin(String name,long bytes,String sha256) {
            if (!name.matches("[A-Za-z0-9_.-]+") || name.equals(".") || name.equals("..")
                    || bytes < 0 || !sha256.matches("[a-f0-9]{64}")) throw new IllegalArgumentException("invalid packaged audio pin");
            this.name=name; this.bytes=bytes; this.sha256=sha256;
        }
    }
    static void directory(File path) throws IOException {
        if (Files.isSymbolicLink(path.toPath())) throw new IOException("audio directory is a link");
        Files.createDirectories(path.toPath());
        if (!Files.isDirectory(path.toPath(),LinkOption.NOFOLLOW_LINKS)) throw new IOException("audio directory unavailable");
    }
    static String digest(MessageDigest digest) {
        StringBuilder value=new StringBuilder();
        for(byte b:digest.digest()) value.append(String.format(Locale.ROOT,"%02x",b & 255));
        return value.toString();
    }
    static MessageDigest hash() {
        try { return MessageDigest.getInstance("SHA-256"); }
        catch(NoSuchAlgorithmException impossible) { throw new AssertionError(impossible); }
    }
    static void active(BooleanSupplier current) throws IOException {
        if (Thread.currentThread().isInterrupted() || !current.getAsBoolean()) throw new IOException("audio preparation cancelled");
    }
    static void verify(File file,Pin pin,BooleanSupplier current,LongConsumer progress) throws IOException {
        if (!Files.isRegularFile(file.toPath(),LinkOption.NOFOLLOW_LINKS) || file.length() != pin.bytes)
            throw new IOException("existing audio input differs: "+pin.name);
        MessageDigest hash=hash();
        try(InputStream input=Files.newInputStream(file.toPath(),LinkOption.NOFOLLOW_LINKS)) {
            byte[] buffer=new byte[65536]; int count;
            while((count=input.read(buffer)) != -1) { active(current); hash.update(buffer,0,count); progress.accept(count); }
        }
        if (!digest(hash).equals(pin.sha256)) throw new IOException("existing audio input differs: "+pin.name);
    }
    static void install(PackagedPayload.Assets assets,String prefix,List<Pin> pins,File destination,
            File stage,BooleanSupplier current,LongConsumer progress) throws IOException {
        directory(destination);
        for(Pin pin:pins) {
            active(current);
            File target=new File(destination,pin.name);
            if (Files.exists(target.toPath(),LinkOption.NOFOLLOW_LINKS)) { verify(target,pin,current,progress);continue; }
            MessageDigest hash=hash(); long length=0;
            try {
                try(InputStream input=assets.open(prefix+"/"+pin.name);
                    FileChannel channel=FileChannel.open(stage.toPath(),StandardOpenOption.CREATE_NEW,
                        StandardOpenOption.WRITE,LinkOption.NOFOLLOW_LINKS);
                    OutputStream output=Channels.newOutputStream(channel)) {
                    byte[] buffer=new byte[65536]; int count;
                    while((count=input.read(buffer)) != -1) {
                        active(current); length+=count;
                        if (length > pin.bytes) throw new IOException("packaged audio length differs: "+pin.name);
                        output.write(buffer,0,count); hash.update(buffer,0,count);progress.accept(count);
                    }
                    if (length != pin.bytes || !digest(hash).equals(pin.sha256)) throw new IOException("packaged audio digest differs: "+pin.name);
                    channel.force(true);
                }
                active(current);
                Files.move(stage.toPath(),target.toPath());
            } finally {
                // This exact stage belongs to the guarded installer, never a model.
                Files.deleteIfExists(stage.toPath());
            }
        }
    }
    // One owner across Activity generations. A canceled predecessor must finish
    // its cleanup before the next Activity can reclaim or reuse the same stage.
    static synchronized void prepare(PackagedPayload.Assets assets,List<Pin> models,List<Pin> notices,
            File destination,File staging,BooleanSupplier current,Runnable ready) throws IOException {
        prepare(assets,models,notices,destination,staging,current,ready,count->{});
    }
    static synchronized void prepare(PackagedPayload.Assets assets,List<Pin> models,List<Pin> notices,
            File destination,File staging,BooleanSupplier current,Runnable ready,LongConsumer progress) throws IOException {
        active(current); directory(staging);
        File marker=new File(staging,"owned-v1");
        if (!Files.exists(marker.toPath(),LinkOption.NOFOLLOW_LINKS)) {
            String[] entries=staging.list();
            if (entries == null || entries.length != 0) throw new IOException("unowned audio staging directory");
            // mkdir is atomic; a crash before it leaves an empty, adoptable parent.
            Files.createDirectory(marker.toPath());
        }
        if (Files.isSymbolicLink(marker.toPath()) || !Files.isDirectory(marker.toPath(),LinkOption.NOFOLLOW_LINKS))
            throw new IOException("unsafe audio staging owner");
        File stage=new File(marker,"payload.stage");
        if (Files.exists(stage.toPath(),LinkOption.NOFOLLOW_LINKS)) {
            if (!Files.isRegularFile(stage.toPath(),LinkOption.NOFOLLOW_LINKS)) throw new IOException("unsafe audio stage");
            Files.delete(stage.toPath());
        }
        install(assets,"audio-models",models,destination,stage,current,progress);
        install(assets,"audio-models/notices",notices,new File(destination,"notices"),stage,current,progress);
        active(current); ready.run();
    }
}
