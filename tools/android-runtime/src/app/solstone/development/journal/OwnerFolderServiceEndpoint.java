// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import android.app.Service;
import android.net.Uri;
import android.os.Bundle;
import android.os.Message;
import android.os.Messenger;
import android.provider.DocumentsContract;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.Executor;
import java.util.function.BooleanSupplier;
import org.json.JSONObject;

/** Typed, private one-shot folder operations on the service's existing workers. */
final class OwnerFolderServiceEndpoint {
    interface Reply { void send(Messenger target, int id, String nativeResult); }
    private final Service service;
    private final Executor workers;
    private final BooleanSupplier destroyed;
    private final Reply reply;

    OwnerFolderServiceEndpoint(Service service, Executor workers, BooleanSupplier destroyed, Reply reply) {
        this.service = service;
        this.workers = workers;
        this.destroyed = destroyed;
        this.reply = reply;
    }

    private String refused() {
        try { return new JSONObject().put("ok", false).put("state", "refused")
            .put("error", OwnerFolderCopy.DENIED).toString(); }
        catch (Exception impossible) { throw new IllegalStateException(impossible); }
    }

    boolean handle(Message message) {
        final Bundle data = message.getData();
        final String op = data.getString("op");
        final Messenger target = message.replyTo;
        final int id = message.arg1;
        if ("owner-folder-page-retire".equals(op)) {
            // Atomic signal only; it never waits behind a provider or Root join.
            Native.retireFolderPage(data.getLong("page_authority"));
            return true;
        }
        if (!"owner-folder-page".equals(op) && !"owner-folder-copy".equals(op)) return false;
        if (destroyed.getAsBoolean()) { reply.send(target,id,refused()); return true; }
        if ("owner-folder-page".equals(op)) {
            final String root = data.getString("root");
            final long generation = data.getLong("admission_generation", -1);
            submit(target,id,() -> {
                long page = Native.issueFolderPage(root,generation);
                return new JSONObject().put("ok",page != 0)
                    .put("value",new JSONObject().put("page_authority",page)).toString();
            });
            return true;
        }
        final byte[] request = data.getByteArray("request");
        final Uri selected = data.getParcelable("tree");
        final long page = data.getLong("page_authority");
        // Remembered connection/configuration has its own explicit owner flow;
        // this endpoint must not silently accept or persist that request.
        if (request == null || request.length == 0 || request.length > 32*1024
                || selected == null || !DocumentsContract.isTreeUri(selected)
                || page == 0 || data.getBoolean("remember",false)) {
            reply.send(target,id,refused());
            return true;
        }
        submit(target,id,() -> {
            JSONObject fields = new JSONObject(new String(request,StandardCharsets.UTF_8));
            if (fields.getLong("page_authority") != page) return refused();
            OwnerFolderServiceCopy copier = new OwnerFolderServiceCopy(service.getContentResolver(),selected,page);
            byte[] result = Native.copyOwnerFolder(service.getCacheDir().getAbsolutePath(),request,copier);
            return result == null ? unknown() : new String(result,StandardCharsets.UTF_8);
        });
        return true;
    }

    private String unknown() {
        try { return new JSONObject().put("ok",false).put("state","outcome_unknown")
            .put("error","this import couldn't be confirmed as finished.").toString(); }
        catch (Exception impossible) { throw new IllegalStateException(impossible); }
    }

    private interface Operation { String run() throws Exception; }
    private void submit(Messenger target,int id,Operation operation) {
        try {
            workers.execute(() -> {
                if (destroyed.getAsBoolean()) { reply.send(target,id,refused()); return; }
                String result;
                try { result = operation.run(); }
                catch (Exception failed) { result = unknown(); }
                // A disconnected Activity does not detach provider work: this
                // happens only after Native returns and its cleanup has retired.
                reply.send(target,id,result);
            });
        } catch (RuntimeException unavailable) { reply.send(target,id,refused()); }
    }
}
