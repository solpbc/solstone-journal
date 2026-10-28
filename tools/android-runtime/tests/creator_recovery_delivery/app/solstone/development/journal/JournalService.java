// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import android.os.*;
import java.io.File;
import java.util.concurrent.*;

/** Only platform/native scheduling is doubled; endpoint, state and caller are real. */
final class JournalService {
    final File files;
    volatile boolean destroyed;
    final BlockingQueue<Runnable> queued=new LinkedBlockingQueue<>();
    final Executor work=queued::add;
    final Execution execution=new Execution();
    final Admission thinkingAdmission=new Admission();
    final CreatorRecoveryEndpoint recovery;
    final Messenger incoming;
    volatile Message lastRequest,lastReply;
    volatile boolean dropReply;
    int recycledId;
    JournalService(File files) {
        this.files=files;
        recovery=new CreatorRecoveryEndpoint(this);
        incoming=new Messenger(new Handler(Looper.getMainLooper()) {
            public void handleMessage(Message message) {
                synchronized(JournalService.this) {
                    lastRequest=message;
                    // Keep the real caller's reply Binder, while allowing captured replies.
                    final Messenger recipient=message.replyTo;
                    message.replyTo=new Messenger(new Handler(Looper.getMainLooper()) {
                        public void handleMessage(Message response) {
                            lastReply=response;
                            if(!dropReply)recipient.send(response);
                        }
                    }) { public IBinder getBinder() { return recipient.getBinder(); } };
                    if(!recovery.handle(message))throw new AssertionError("wrong endpoint");
                    // Looper recycles after dispatchMessage returns. Clear fields before work.
                    message.arg1=recycledId;message.sendingUid=-1;message.replyTo=null;
                    message.setData(null);
                }
            }
        });
    }
    File getFilesDir() { return files; }
    void cancelNativeWork() { Native.epoch+=2; }
    void runNext() throws Exception {
        Runnable next=queued.poll(5,TimeUnit.SECONDS);
        if(next==null)throw new AssertionError("maintained endpoint did not enqueue work");
        synchronized(this) {
            if(lastRequest.arg1!=recycledId||lastRequest.replyTo!=null)throw new AssertionError("request not recycled before work");
            next.run();
        }
    }
    static final class Execution { void beginStop(Runnable cancel) { cancel.run(); } }
    static final class Admission { void awaitIdle() {} }
}
