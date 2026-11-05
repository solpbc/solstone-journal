// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import android.app.AlertDialog;
import android.content.Intent;
import android.net.Uri;
import android.webkit.URLUtil;
import java.io.*;
import java.util.UUID;
import org.json.JSONArray;

/** Owner-selected copies, streamed through private storage and the system picker. */
final class OwnerDownloads {
    static final int PICK_SAVE = 43;
    final OwnerActivity activity;
    Transfer active, picker;
    final class Transfer {
        final String id=UUID.randomUUID().toString(), token, name, mime;
        final File file;
        long expected;
        OutputStream staging;
        volatile boolean cancelled;
        volatile Closeable source, destination;
        OwnerTransport.Cancellation request;
        long bytes;
        boolean complete, choosing;
        AlertDialog progress;
        Transfer(String token,String name,String mime,long expected) throws IOException {
            this.token=token;this.name=name;this.mime=mime;this.expected=expected;
            file=File.createTempFile("journal-export-",".tmp",activity.getCacheDir());
            staging=new FileOutputStream(file);
        }
    }
    OwnerDownloads(OwnerActivity activity) { this.activity=activity; }
    boolean current(Transfer transfer) {
        return transfer != null && active == transfer && !transfer.cancelled
            && transfer.token.equals(activity.pageToken) && !activity.ownerStopped && !activity.destroyed;
    }
    synchronized Transfer begin(String token,String name,String mime,long expected) throws IOException {
        if (active != null || picker != null || token.isEmpty() || !token.equals(activity.pageToken) || activity.ownerStopped)
            throw new IOException("export unavailable");
        String leaf=new File(name).getName();
        if (leaf.isEmpty() || leaf.startsWith(".") || leaf.length()>200 || leaf.matches(".*[\\p{Cntrl}\\\\/].*")) leaf="journal-export";
        String type=mime != null && mime.matches("[a-zA-Z0-9.+-]+/[a-zA-Z0-9.+-]+") ? mime : "application/octet-stream";
        active=new Transfer(token,leaf,type,expected);
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
        transfer.staging.close();transfer.staging=null;transfer.complete=true;
        activity.ui.post(()->choose(transfer));
    }
    void receive(String url,String disposition,String mime) {
        final Transfer transfer;
        final OwnerTransport transport=activity.transport;
        try {
            OwnerOrigin.admit(url);
            if (transport == null) throw new IOException("journal unavailable");
            transfer=begin(activity.pageToken,URLUtil.guessFileName(url,disposition,mime),mime,-1);
            transfer.request=transport.new Cancellation();
        } catch(Exception error) { failed();return; }
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
        if(!transfer.file.delete() && transfer.file.exists()) android.util.Log.w("JournalExport","temporary export cleanup failed");
        if(active==transfer) active=null;
        activity.ui.post(()->dismissProgress(transfer));
    }
    void dismissProgress(Transfer transfer) { if(transfer.progress!=null) {transfer.progress.dismiss();transfer.progress=null;} }
    synchronized void retire() { cancel(active); }
}
