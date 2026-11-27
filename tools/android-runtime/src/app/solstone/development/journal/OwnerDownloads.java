// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import android.app.AlertDialog;
import android.content.Intent;
import android.net.Uri;
import android.provider.DocumentsContract;
import android.webkit.URLUtil;
import java.io.*;
import java.security.MessageDigest;
import java.util.UUID;
import org.json.JSONArray;

/** Owner-selected copies, streamed directly to SAF for prepared exports or through private storage for blobs. */
final class OwnerDownloads {
    static final int PICK_SAVE = 43;
    final OwnerActivity activity;
    Transfer active, picker;
    final class Transfer {
        final String id=UUID.randomUUID().toString(), token, name, mime, sourceUrl;
        final boolean directStream;
        final File file;
        long expected;
        String expectedSha256;
        OutputStream staging;
        volatile boolean cancelled;
        volatile Closeable source, destination;
        OwnerTransport.Cancellation request;
        long bytes;
        boolean complete, choosing;
        AlertDialog progress;
        Transfer(String token,String name,String mime,long expected,String sourceUrl,boolean directStream) throws IOException {
            this.token=token;this.name=name;this.mime=mime;this.expected=expected;
            this.sourceUrl=sourceUrl;this.directStream=directStream;
            if (directStream) {
                file=null;
                staging=null;
            } else {
                file=File.createTempFile("journal-export-",".tmp",activity.getCacheDir());
                staging=new FileOutputStream(file);
            }
        }
    }
    OwnerDownloads(OwnerActivity activity) {
        this.activity=activity;
        sweepStaleTemporaryFiles(activity.getCacheDir());
    }
    static void sweepStaleTemporaryFiles(File cacheDir) {
        if (cacheDir == null || !cacheDir.isDirectory()) return;
        File[] files = cacheDir.listFiles();
        if (files == null) return;
        for (File f : files) {
            String n = f.getName();
            if (n.startsWith("journal-export-") && n.endsWith(".tmp")) {
                if (!f.delete() && f.exists()) {
                    android.util.Log.w("JournalExport", "stale export temp cleanup failed");
                }
            }
        }
    }
    static boolean isPreparedExportUrl(String url) {
        if (url == null) return false;
        int q=url.indexOf('?');
        String clean=q >= 0 ? url.substring(0,q) : url;
        return clean.contains("/app/export/api/ready/") && clean.endsWith("/download");
    }
    static String hexDigest(byte[] digest) {
        StringBuilder out=new StringBuilder(digest.length*2);
        for(byte b:digest) out.append(String.format("%02x",b & 0xff));
        return out.toString();
    }
    boolean current(Transfer transfer) {
        return transfer != null && active == transfer && !transfer.cancelled
            && transfer.token.equals(activity.pageToken) && !activity.ownerStopped && !activity.destroyed;
    }
    synchronized Transfer begin(String token,String name,String mime,long expected) throws IOException {
        return beginWithMode(token,name,mime,expected,null,false);
    }
    synchronized Transfer beginWithMode(String token,String name,String mime,long expected,String sourceUrl,boolean directStream) throws IOException {
        if (active != null || picker != null || token.isEmpty() || !token.equals(activity.pageToken) || activity.ownerStopped)
            throw new IOException("export unavailable");
        String leaf=new File(name).getName();
        if (leaf.isEmpty() || leaf.startsWith(".") || leaf.length()>200 || leaf.matches(".*[\\p{Cntrl}\\\\/].*")) leaf="journal-export";
        String type=mime != null && mime.matches("[a-zA-Z0-9.+-]+/[a-zA-Z0-9.+-]+") ? mime : "application/octet-stream";
        active=new Transfer(token,leaf,type,expected,sourceUrl,directStream);
        return active;
    }
    synchronized void append(Transfer transfer,byte[] bytes,int count) throws IOException {
        if (!current(transfer) || transfer.complete || count<0 || count>65536
                || transfer.expected>=0 && count>transfer.expected-transfer.bytes) throw new IOException("export changed");
        transfer.staging.write(bytes,0,count);
        transfer.bytes+=count;
    }
    synchronized void ready(Transfer transfer) throws IOException {
        if (!current(transfer) || transfer.complete || transfer.expected>=0 && transfer.bytes!=transfer.expected)
            throw new IOException("export incomplete");
        if (transfer.staging!=null) { transfer.staging.close();transfer.staging=null; }
        transfer.complete=true;
        activity.ui.post(()->choose(transfer));
    }
    void receive(String url,String disposition,String mime) {
        final Transfer transfer;
        final OwnerTransport transport=activity.transport;
        final boolean direct=isPreparedExportUrl(url);
        try {
            OwnerOrigin.admit(url);
            if (transport == null) throw new IOException("journal unavailable");
            transfer=beginWithMode(activity.pageToken,URLUtil.guessFileName(url,disposition,mime),mime,-1,url,direct);
            transfer.request=transport.new Cancellation();
        } catch(Exception error) { failed();return; }
        if (direct) {
            // Prepared exports live already staged by the journal: probe headers via HEAD,
            // open the SAF picker first with zero cache copy, then stream once into the chosen URI.
            try { activity.uploads.execute(()->{
                try(OwnerTransport.Stream head=transport.follow("HEAD",url,"",new JSONArray(),transfer.request)) {
                    transfer.source=head;
                    if (head.status!=200) throw new IOException("export response refused");
                    long expected=Long.parseLong(head.headers.get("content-length"));
                    if(expected<0) throw new IOException("export length unavailable");
                    String sha=head.headers.get("x-solstone-export-sha256");
                    synchronized(this) {
                        if(!current(transfer)) throw new IOException("export cancelled");
                        transfer.expected=expected;
                        transfer.expectedSha256=sha;
                        transfer.source=null;
                    }
                    activity.ui.post(()->choose(transfer));
                } catch(Exception error) { fail(transfer); }
            }); } catch(Exception error) { fail(transfer); }
            return;
        }
        transfer.progress=new AlertDialog.Builder(activity).setMessage("preparing file…")
            .setNegativeButton(android.R.string.cancel,(dialog,which)->cancel(transfer))
            .setOnCancelListener(dialog->cancel(transfer)).show();
        try { activity.uploads.execute(()->{
            try(OwnerTransport.Stream stream=transport.follow("GET",url,"",new JSONArray(),transfer.request)) {
                transfer.source=stream;
                if (stream.status!=200) throw new IOException("export response refused");
                long expected=Long.parseLong(stream.headers.get("content-length"));
                if(expected<0) throw new IOException("export length unavailable");
                synchronized(this) { transfer.expected=expected; }
                byte[] bytes=new byte[65536];int count;
                while((count=stream.read(bytes))!=-1) append(transfer,bytes,count);
                if (expected>=0 && expected!=transfer.bytes) throw new IOException("export incomplete");
                ready(transfer);
            } catch(Exception error) { fail(transfer); }
        }); } catch(Exception error) { fail(transfer); }
    }
    synchronized String beginBlob(String token,String name,String mime,long bytes) {
        try { if(bytes<0) return "";return begin(token,name,mime,bytes).id; }
        catch(Exception error) { activity.ui.post(this::failed);return ""; }
    }
    synchronized boolean blobChunk(String token,String id,String encoded) {
        Transfer transfer=active;
        if (transfer==null || !transfer.id.equals(id) || !transfer.token.equals(token)) return false;
        try {
            if(encoded.length()>88000) throw new IOException("export chunk too large");
            byte[] bytes=android.util.Base64.decode(encoded,android.util.Base64.DEFAULT);
            append(transfer,bytes,bytes.length);return true;
        } catch(Exception error) { fail(transfer);return false; }
    }
    synchronized void endBlob(String token,String id,boolean success) {
        Transfer transfer=active;
        if (transfer==null || !transfer.id.equals(id) || !transfer.token.equals(token)) return;
        try { if(!success) throw new IOException("export interrupted");ready(transfer); }
        catch(Exception error) { fail(transfer); }
    }
    synchronized void choose(Transfer transfer) {
        dismissProgress(transfer);
        if(!current(transfer)) { cancel(transfer);return; }
        try {
            transfer.choosing=true;picker=transfer;
            activity.startActivityForResult(new Intent(Intent.ACTION_CREATE_DOCUMENT)
                .addCategory(Intent.CATEGORY_OPENABLE).setType(transfer.mime)
                .putExtra(Intent.EXTRA_TITLE,transfer.name),PICK_SAVE);
        } catch(Exception error) { picker=null;fail(transfer); }
    }
    void selected(int result,Intent data) {
        final Transfer transfer;
        synchronized(this) {
            transfer=picker;picker=null;
            if (transfer==null || !transfer.choosing) return;
            transfer.choosing=false;
            if(result!=android.app.Activity.RESULT_OK || data==null || data.getData()==null || !current(transfer)) {
                cancel(transfer);return;
            }
        }
        final Uri uri=data.getData();
        if (transfer.directStream) {
            final OwnerTransport transport=activity.transport;
            if (transport == null) { fail(transfer); return; }
            transfer.request=transport.new Cancellation();
            try { activity.uploads.execute(()->{
                boolean verified=false;
                try(OwnerTransport.Stream stream=transport.follow("GET",transfer.sourceUrl,"",new JSONArray(),transfer.request);
                    OutputStream output=activity.getContentResolver().openOutputStream(uri,"w")) {
                    if(stream.status!=200) throw new IOException("export response refused");
                    if(output==null) throw new IOException("destination unavailable");
                    synchronized(this) {
                        if(!current(transfer)) throw new IOException("export cancelled");
                        transfer.source=stream;
                        transfer.destination=output;
                    }
                    MessageDigest sha=MessageDigest.getInstance("SHA-256");
                    byte[] bytes=new byte[65536];int count;long total=0;
                    while((count=stream.read(bytes))!=-1) {
                        synchronized(this) { if(!current(transfer)) throw new IOException("export cancelled"); }
                        if(transfer.expected>=0 && total+count>transfer.expected) throw new IOException("export changed");
                        output.write(bytes,0,count);
                        sha.update(bytes,0,count);
                        total+=count;
                    }
                    if(transfer.expected>=0 && total!=transfer.expected) throw new IOException("export incomplete");
                    String expectedSha=transfer.expectedSha256;
                    if(expectedSha!=null && !expectedSha.isEmpty()) {
                        String actualSha=hexDigest(sha.digest());
                        if(!expectedSha.equalsIgnoreCase(actualSha)) throw new IOException("export checksum mismatch");
                    }
                    output.flush();
                    verified=true;
                } catch(Exception error) {
                    try { DocumentsContract.deleteDocument(activity.getContentResolver(), uri); } catch(Exception ignored) {}
                    fail(transfer);return;
                }
                if (!verified) return;
                activity.ui.post(()->{
                    synchronized(this) {
                        if(current(transfer)) activity.notice("copy saved to the folder you chose");
                        cancel(transfer);
                    }
                });
            }); } catch(Exception error) { fail(transfer); }
            return;
        }
        try { activity.uploads.execute(()->{
            try(InputStream input=new FileInputStream(transfer.file);
                OutputStream output=activity.getContentResolver().openOutputStream(uri,"w")) {
                if(output==null) throw new IOException("destination unavailable");
                synchronized(this) {
                    if(!current(transfer)) throw new IOException("export cancelled");
                    transfer.destination=output;
                }
                byte[] bytes=new byte[65536];int count;
                while((count=input.read(bytes))!=-1) {
                    synchronized(this) { if(!current(transfer)) throw new IOException("export cancelled"); }
                    output.write(bytes,0,count);
                }
            } catch(Exception error) { fail(transfer);return; }
            activity.ui.post(()->{
                synchronized(this) {
                    if(current(transfer)) activity.notice("file saved");
                    cancel(transfer);
                }
            });
        }); } catch(Exception error) { fail(transfer); }
    }
    void failed() { if(!activity.destroyed) activity.notice("couldn't save this file. a partial copy may remain; try saving it again."); }
    synchronized void fail(Transfer transfer) {
        boolean report=current(transfer);
        cancel(transfer);
        if(report) activity.ui.post(this::failed);
    }
    synchronized void cancel(Transfer transfer) {
        if(transfer==null) return;
        transfer.cancelled=true;
        if(transfer.request!=null) transfer.request.cancel();
        for(Closeable resource:new Closeable[]{transfer.staging,transfer.source,transfer.destination}) {
            if(resource!=null) try { resource.close(); } catch(IOException error) { android.util.Log.w("JournalExport","export close failed",error); }
        }
        if(transfer.file!=null && !transfer.file.delete() && transfer.file.exists()) android.util.Log.w("JournalExport","temporary export cleanup failed");
        if(active==transfer) active=null;
        activity.ui.post(()->dismissProgress(transfer));
    }
    void dismissProgress(Transfer transfer) { if(transfer.progress!=null) {transfer.progress.dismiss();transfer.progress=null;} }
    synchronized void retire() { cancel(active); }
}
