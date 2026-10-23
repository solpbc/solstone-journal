// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import android.os.*;
import android.system.Os;
import android.system.OsConstants;
import org.json.*;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.util.*;
import java.util.concurrent.*;
import java.util.concurrent.atomic.AtomicLong;

/** The service is non-exported; no network listener or external IPC has owner authority. */
final class OwnerTransport implements Closeable {
    final ServingActivity activity;
    final long generation;
    final long nativeGeneration;
    final Messenger worker;
    static final AtomicLong identities = new AtomicLong();
    final ConcurrentHashMap<Long,Stream> streams = new ConcurrentHashMap<>();
    final ConcurrentHashMap<Long,CompletableFuture<JSONObject>> opening = new ConcurrentHashMap<>();
    volatile boolean closed;
    final class Cancellation {
        volatile boolean cancelled;
        long id;
        CompletableFuture<JSONObject> waiting;
        Stream stream;
        synchronized void check() throws IOException {
            if (cancelled) throw new IOException("journal request cancelled; outcome may be unknown");
        }
        synchronized void cancel() {
            cancelled=true;
            if (waiting != null) waiting.completeExceptionally(new IOException("journal request cancelled"));
            if (id != 0) OwnerTransport.this.cancel(id);
            if (stream != null) try { stream.close(); } catch(IOException ignored) {}
        }
    }
    OwnerTransport(ServingActivity activity) {
        this.activity=activity;
        this.generation=activity.connectionGeneration.get();
        this.nativeGeneration=activity.ownerGeneration;
        this.worker=activity.worker;
    }
    boolean current() {
        return !closed && !activity.ownerStopped && nativeGeneration >= 0
            && nativeGeneration == activity.ownerGeneration && activity.currentConnection(generation,worker);
    }
    final class Stream extends FilterInputStream {
        final long id;
        final int status;
        final Map<String,String> headers;
        final String url;
        boolean retired;
        Stream(long id,ParcelFileDescriptor source,int status,Map<String,String> headers,String url) {
            super(new ParcelFileDescriptor.AutoCloseInputStream(source));
            this.id=id; this.status=status; this.headers=headers; this.url=url;
        }
        public synchronized void close() throws IOException {
            if (retired) return;
            retired=true;
            streams.remove(id);
            cancel(id);
            super.close();
        }
    }
    void cancel(long id) {
        if (worker == null) return;
        Message message=Message.obtain(null,1);
        Bundle bundle=new Bundle(); bundle.putString("op","owner-cancel"); bundle.putLong("owner_id",id);
        message.setData(bundle);
        try { worker.send(message); } catch(RemoteException ignored) {}
    }
    Stream open(String method,String url,String body,JSONArray headers) throws Exception {
        return open(method,url,body,headers,new Cancellation());
    }
    Stream open(String method,String url,String body,JSONArray headers,Cancellation cancellation) throws Exception {
        cancellation.check();
        String uri=OwnerOrigin.path(url);
        if (!current() || streams.size() >= 32) throw new IOException("journal controls unavailable");
        long ownerId=identities.incrementAndGet();
        JSONObject request=MainActivity.obj("id",ownerId,"method",method,"uri",uri,"body",body,"headers",headers,
            "admission_generation",nativeGeneration);
        byte[] bytes=request.toString().getBytes(StandardCharsets.UTF_8);
        if (bytes.length > 384*1024) throw new IOException("journal request too large");
        ParcelFileDescriptor[] pipe=ParcelFileDescriptor.createPipe();
        CompletableFuture<JSONObject> result=new CompletableFuture<>();
        int id=activity.sequence.incrementAndGet();
        activity.pending.put(id,result);
        opening.put(ownerId,result);
        boolean returned=false;
        try {
            Os.fcntlInt(pipe[1].getFileDescriptor(),OsConstants.F_SETFL,OsConstants.O_NONBLOCK);
            Message message=Message.obtain(null,1); message.arg1=id; message.replyTo=activity.replies;
            Bundle bundle=new Bundle(); bundle.putString("op","owner-open"); bundle.putString("root",activity.root);
            bundle.putByteArray("request",bytes); bundle.putParcelable("sink",pipe[1]); message.setData(bundle);
            synchronized(cancellation) {
                cancellation.check();
                cancellation.id=ownerId; cancellation.waiting=result;
                worker.send(message);
            }
            // Binder duplicates the descriptor before send returns.
            pipe[1].close();
            JSONObject nativeResult=result.get(70,TimeUnit.SECONDS).getJSONObject("native");
            if (!nativeResult.optBoolean("ok")) throw new IOException(nativeResult.optString("error","journal request failed"));
            if (!current()) throw new IOException("journal connection changed; outcome may be unknown");
            JSONObject head=nativeResult.getJSONObject("value");
            Map<String,String> responseHeaders=new TreeMap<>(String.CASE_INSENSITIVE_ORDER);
            JSONArray pairs=head.getJSONArray("headers");
            for(int i=0;i<pairs.length();i++) {
                JSONArray pair=pairs.getJSONArray(i); responseHeaders.put(pair.getString(0),pair.getString(1));
            }
            Stream stream=new Stream(ownerId,pipe[0],head.getInt("status"),responseHeaders,url);
            streams.put(ownerId,stream);
            synchronized(cancellation) {
                cancellation.stream=stream;
                if (cancellation.cancelled) { stream.close(); throw new IOException("journal request cancelled"); }
            }
            if (!current()) { stream.close(); throw new IOException("journal connection changed"); }
            returned=true;
            return stream;
        } finally {
            activity.pending.remove(id);
            opening.remove(ownerId);
            pipe[1].close();
            if (!returned) { cancel(ownerId); pipe[0].close(); }
        }
    }
    Stream follow(String method,String url,String body,JSONArray headers) throws Exception {
        return follow(method,url,body,headers,new Cancellation());
    }
    Stream follow(String method,String url,String body,JSONArray headers,Cancellation cancellation) throws Exception {
        for(int redirects=0; redirects<10; redirects++) {
            Stream stream=open(method,url,body,headers,cancellation);
            if (!Arrays.asList(301,302,303,307,308).contains(stream.status)) return stream;
            String location=stream.headers.get("location");
            int status=stream.status; stream.close();
            if (location == null) throw new IOException("journal redirect has no address");
            url=OwnerOrigin.redirect(url,location);
            if (status == 303 || ((status == 301 || status == 302) && "POST".equals(method))) {
                method="GET"; body=""; headers=new JSONArray();
            }
        }
        throw new IOException("too many journal redirects");
    }
    static byte[] readBounded(InputStream stream,int limit) throws IOException {
        ByteArrayOutputStream bytes=new ByteArrayOutputStream();
        byte[] buffer=new byte[16384]; int count;
        while((count=stream.read(buffer)) != -1) {
            if (bytes.size()+count > limit) throw new IOException("journal response too large");
            bytes.write(buffer,0,count);
        }
        return bytes.toByteArray();
    }
    public void close() {
        closed=true;
        for(Map.Entry<Long,CompletableFuture<JSONObject>> entry:opening.entrySet()) {
            cancel(entry.getKey());
            entry.getValue().completeExceptionally(new IOException("journal page closed; outcome may be unknown"));
        }
        for(Stream stream:streams.values()) try { stream.close(); } catch(IOException ignored) {}
    }
}
