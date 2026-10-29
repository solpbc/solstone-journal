// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;
import android.app.Service;
import android.app.ActivityManager;
import android.content.ComponentName;
import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.os.*;
import android.os.Process;
import org.json.*;
import java.util.concurrent.*;
import java.io.*;
import java.nio.charset.StandardCharsets;
public class JournalService extends Service {
    final JournalExecution execution = JournalExecution.PROCESS;
    final ExecutorService work=execution.work;
    final ExecutorService thinking=execution.thinking;
    final ThinkingAdmission thinkingAdmission = execution.admission;
    volatile boolean destroyed;
    OwnerFolderServiceEndpoint folderRequests;
    SavedFolderServiceProvider savedFolderProvider;
    long savedFolderProviderIdentity;
    String serviceToolAdmission;
    Messenger incoming;
    final Handler setupHandler=new Handler(Looper.getMainLooper());
    final SetupForeground setupForeground=new SetupForeground(SystemClock::elapsedRealtime);
    final ExecutorService ownerRequests = new ThreadPoolExecutor(4,4,0L,TimeUnit.MILLISECONDS,
        new ArrayBlockingQueue<Runnable>(32),new ThreadPoolExecutor.AbortPolicy());
    public void onCreate() {
        super.onCreate();
        savedFolderProvider = new SavedFolderServiceProvider(getContentResolver());
        savedFolderProviderIdentity = Native.installSavedFolderProvider(getCacheDir().getAbsolutePath(),savedFolderProvider);
        long admissionStarted = SystemClock.elapsedRealtime();
        evidence("service_tools_admission_started", "");
        try { serviceToolAdmission = ServiceToolAssets.install(this); }
        catch (Exception error) {
            serviceToolAdmission = "{\"ok\":false,\"packaged\":true,\"error\":" + JSONObject.quote(error.toString()) + "}";
        }
        evidence("service_tools_admission_finished", Long.toString(SystemClock.elapsedRealtime() - admissionStarted));
        folderRequests = new OwnerFolderServiceEndpoint(this,ownerRequests,() -> destroyed,
            (target,id,result) -> respond(target,id,result,0));
        incoming=new Messenger(new Handler(Looper.getMainLooper()) {
            public void handleMessage(Message m) {
                if (folderRequests.handle(m)) return;
                final String op=m.getData().getString("op");
                final String root=m.getData().getString("root");
                final int count=m.getData().getInt("count",1), id=m.arg1;
                final Messenger reply=m.replyTo;
                if ("owner-setup-end".equals(op)) {
                    if (m.sendingUid == Process.myUid()) setupForeground.end(m.getData().getString("lease"));
                    return;
                }
                if ("owner-setup-begin".equals(op)) {
                    final String lease=m.getData().getString("lease");
                    boolean acquired=false;
                    try {
                        boolean admitted=!destroyed && m.sendingUid == Process.myUid() && reply != null
                            && thinkingAdmission.isOpen() && lease != null && lease.length() == 36;
                        JSONObject generation=new JSONObject(Native.run("", "thinking-generation", 1));
                        admitted=admitted && generation.optBoolean("ok")
                            && generation.getJSONObject("value").getLong("generation") == m.getData().getLong("generation",-1);
                        if (admitted) acquired=setupForeground.begin(lease,true,setupEffects(lease,reply.getBinder()));
                    } catch(Exception error) { /* Refuse protection; no browser dispatch follows. */ }
                    if (reply != null) {
                        Message answer=Message.obtain(null,1); Bundle data=new Bundle();
                        data.putString("lease",lease); data.putBoolean("acquired",acquired); answer.setData(data);
                        try { reply.send(answer); }
                        catch(RemoteException error) { setupForeground.end(lease); }
                    }
                    return;
                }
                if ("owner-cancel".equals(op)) {
                    Native.cancelOwner(m.getData().getLong("owner_id"));
                    return;
                }
                if ("owner-poll".equals(op)) {
                    byte[] result = Native.pollOwner(m.getData().getLong("owner_id"));
                    respond(reply,id,new String(result,StandardCharsets.UTF_8),0);
                    return;
                }
                if ("owner-open-stream".equals(op)) {
                    final ParcelFileDescriptor sink = m.getData().getParcelable("sink");
                    final ParcelFileDescriptor body = m.getData().getParcelable("body");
                    final byte[] request = m.getData().getByteArray("request");
                    if (destroyed || sink == null || body == null || request == null || request.length > 384*1024) {
                        if (sink != null) try { sink.close(); } catch(IOException ignored) {}
                        if (body != null) try { body.close(); } catch(IOException ignored) {}
                        respond(reply,id,"{\"ok\":false,\"error\":\"owner request unavailable\"}",0);
                        return;
                    }
                    try {
                        ownerRequests.execute(()->{
                            int responseFd = -1;
                            int bodyFd = -1;
                            try {
                                responseFd = sink.detachFd();
                                bodyFd = body.detachFd();
                                byte[] result = Native.openOwnerStream(root,request,responseFd,bodyFd);
                                respond(reply,id,new String(result,StandardCharsets.UTF_8),0);
                            } catch(Exception error) {
                                respond(reply,id,"{\"ok\":false,\"error\":\"owner response unavailable; outcome may be unknown\"}",0);
                            } finally {
                                try { sink.close(); } catch(IOException ignored) {}
                                try { body.close(); } catch(IOException ignored) {}
                            }
                        });
                    } catch(Exception error) {
                        try { sink.close(); } catch(IOException ignored) {}
                        try { body.close(); } catch(IOException ignored) {}
                        respond(reply,id,"{\"ok\":false,\"error\":\"owner requests busy\"}",0);
                    }
                    return;
                }
                if ("owner-open".equals(op)) {
                    final ParcelFileDescriptor sink = m.getData().getParcelable("sink");
                    final byte[] request = m.getData().getByteArray("request");
                    if (destroyed || sink == null || request == null || request.length > 384*1024) {
                        if (sink != null) try { sink.close(); } catch(IOException ignored) {}
                        respond(reply,id,"{\"ok\":false,\"error\":\"owner request unavailable\"}",0);
                        return;
                    }
                    try {
                        ownerRequests.execute(()->{
                            try {
                                byte[] result=Native.openOwner(root,request,sink.detachFd());
                                respond(reply,id,new String(result,StandardCharsets.UTF_8),0);
                            } catch(Exception error) {
                                respond(reply,id,"{\"ok\":false,\"error\":\"owner response unavailable; outcome may be unknown\"}",0);
                            } finally { try { sink.close(); } catch(IOException ignored) {} }
                        });
                    } catch(Exception error) {
                        try { sink.close(); } catch(IOException ignored) {}
                        respond(reply,id,"{\"ok\":false,\"error\":\"owner requests busy\"}",0);
                    }
                    return;
                }
                if (destroyed) {
                    respond(reply,id,"{\"ok\":false,\"error\":\"service destroyed\"}",0);
                    return;
                }
                if ("owner-stop".equals(op)) {
                    ServingActivity.lifecycle("stop_received");
                    setupForeground.stop(()->execution.beginStop(JournalService.this::cancelNativeWork));
                    ServingActivity.lifecycle("stop_cancellation_delivered");
                    return;
                }
                if (op.equals("cancel") || op.equals("begin")) { respond(reply,id,Native.run(root,op,count),0); return; }
                String operation = "";
                if (op.startsWith("serve:")) {
                    try { operation = new JSONObject(op.substring(6)).optString("op"); }
                    catch (JSONException e) { /* Native boundary returns the parse refusal. */ }
                }
                final boolean isThinking = operation.equals("generate") || operation.equals("talent");
                final boolean isToolStatus = operation.equals("service-tools");
                final boolean isStop = operation.equals("stop"), isStart = operation.equals("start");
                String admitted = op;
                if (isThinking || isStart || operation.equals("audio-enable")) {
                    try {
                        JSONObject nativeGeneration = new JSONObject(Native.run("", "thinking-generation", 1));
                        if (!nativeGeneration.optBoolean("ok")) throw new JSONException("native admission unavailable");
                        JSONObject command = new JSONObject(op.substring(6));
                        command.put("admission_generation", nativeGeneration.getJSONObject("value").getLong("generation"));
                        admitted = "serve:" + command;
                    } catch (JSONException e) {
                        respond(reply,id,"{\"ok\":false,\"error\":\"native admission unavailable\"}",0);
                        return;
                    }
                }
                final String admittedOp = admitted;
                final long generation = thinkingAdmission.generation();
                if (isStop) {
                    setupForeground.stop(()->execution.beginStop(JournalService.this::cancelNativeWork));
                }
                final ThinkingAdmission.Ticket ticket = isThinking ? thinkingAdmission.admit() : null;
                if (isThinking && ticket == null) {
                    respond(reply,id,"{\"ok\":false,\"error\":\"thinking is stopped or busy\"}",0);
                    return;
                }
                try {
                    (isThinking ? thinking : work).execute(()->{
                        long start=SystemClock.elapsedRealtimeNanos();
                        try {
                            if (isStop) {
                                ServingActivity.lifecycle("stop_retirement_requested");
                                thinkingAdmission.awaitIdle();
                            }
                            String result;
                            if (ticket != null && !ticket.current()) {
                                result="{\"ok\":false,\"error\":\"server stopped\"}";
                            } else {
                                result=Native.run(root,admittedOp,count);
                            }
                            if (isStop) {
                                JSONObject nativeStop = new JSONObject(result);
                                JSONObject value = nativeStop.optJSONObject("value");
                                ServingActivity.lifecycle(nativeStop.optBoolean("ok") && value != null && value.optBoolean("stopped")
                                    ? "stop_retirement_complete" : "stop_retirement_unconfirmed");
                            }
                            if (isToolStatus) {
                                JSONObject diagnostic = new JSONObject(result);
                                diagnostic.put("startup", new JSONObject(serviceToolAdmission));
                                result = diagnostic.toString();
                            }
                            if (isStart && new JSONObject(result).optBoolean("ok")) {
                                thinkingAdmission.opened(generation);
                                // Sweep only here, before this start admits the owner page.
                                try { Native.reclaimBridge(getCacheDir().getAbsolutePath(), root, "[]"); }
                                catch (Throwable ignored) { /* A failed cleanup must not hide a started journal. */ }
                            }
                            respond(reply,id,result,SystemClock.elapsedRealtimeNanos()-start);
                        } catch(Throwable e) {
                            respond(reply,id,"{\"ok\":false,\"error\": "+JSONObject.quote(e.toString())+"}",SystemClock.elapsedRealtimeNanos()-start);
                        } finally {
                            if (ticket != null) ticket.close();
                        }
                    });
                } catch (RejectedExecutionException e) {
                    if (ticket != null) ticket.close();
                    respond(reply,id,"{\"ok\":false,\"error\":\"server stopped\"}",0);
                }
            }
        });
    }
    SetupForeground.Effects setupEffects(String lease, IBinder client) {
        return new SetupForeground.Effects() {
            final Runnable timeout=new Runnable() {
                public void run() {
                    long delay=setupForeground.checkTimeout(lease);
                    if (delay > 0 && !setupHandler.postDelayed(this,delay)) setupForeground.end(lease);
                }
            };
            final IBinder.DeathRecipient died=()->setupHandler.post(()->setupForeground.end(lease));
            boolean linked;
            public boolean start() throws Exception {
                client.linkToDeath(died,0); linked=true;
                NotificationManager notifications=getSystemService(NotificationManager.class);
                notifications.createNotificationChannel(new NotificationChannel("journal-setup","journal setup",NotificationManager.IMPORTANCE_LOW));
                PendingIntent back=PendingIntent.getActivity(JournalService.this,0,
                    new Intent(JournalService.this,OwnerActivity.class).addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP|Intent.FLAG_ACTIVITY_SINGLE_TOP),
                    PendingIntent.FLAG_UPDATE_CURRENT|PendingIntent.FLAG_IMMUTABLE);
                Notification notification=new Notification.Builder(JournalService.this,"journal-setup")
                    .setSmallIcon(android.R.drawable.stat_sys_upload).setContentTitle("journal setup")
                    .setContentText("finish setup in your browser, then return to your journal.")
                    .setContentIntent(back).setOngoing(true)
                    .addAction(new Notification.Action.Builder(null,"open journal",back).build()).build();
                // This service remains bound-only. Promotion does not create a
                // started/sticky lifetime or restart a stopped native journal.
                startForeground(7,notification,ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC);
                if (!setupForegroundObserved()) return false;
                // Handler delays exclude sleep. Recheck the elapsed deadline
                // after a bounded awake interval instead of owing a full delay.
                if (!setupHandler.postDelayed(timeout,SetupForeground.CHECK_INTERVAL_MS)) throw new IllegalStateException("setup timer unavailable");
                return true;
            }
            public void finish() {
                setupHandler.removeCallbacks(timeout);
                try { if (linked) client.unlinkToDeath(died,0); }
                catch(java.util.NoSuchElementException ignored) { /* Binder death already removed the watch. */ }
                finally { linked=false; stopForeground(STOP_FOREGROUND_REMOVE); }
            }
        };
    }
    boolean setupForegroundObserved() {
        ActivityManager activity=getSystemService(ActivityManager.class);
        ComponentName expected=new ComponentName(this,JournalService.class);
        // Promotion may be silently ignored by app-ops. A void return is not
        // an acquisition receipt; observe this exact service in system state.
        for (ActivityManager.RunningServiceInfo service : activity.getRunningServices(32)) {
            if (expected.equals(service.service) && service.uid == Process.myUid()
                    && service.pid == Process.myPid()) return service.foreground;
        }
        return false;
    }
    public void onTimeout(int startId,int type) {
        setupForeground.close();
        stopSelf(startId);
    }
    void cancelNativeWork() {
        String result = Native.run("", "cancel", 1);
        // A positive observation follows actual cancellation delivery. No
        // response body, input path, samples, output or credential is logged.
        try {
            JSONObject value = new JSONObject(result).optJSONObject("value");
            if (value != null && value.optBoolean("native_call_active_after_cancel")) {
                ServingActivity.lifecycle("native_asr_active_after_cancel");
                final long observedAt = SystemClock.elapsedRealtime();
                // Persistence must not hold the main Handler or delay retirement.
                // A full/closed queue loses this optional witness, never Stop.
                try {
                    ownerRequests.execute(() -> evidence("native_asr_call_active_after_cancel",
                        "active after Stop cancellation signalled; observed_elapsed_ms=" + observedAt));
                } catch (RejectedExecutionException ignored) { /* No evidence is not proof of idle. */ }
            }
        } catch (JSONException ignored) { /* Unknown observation cannot establish overlap. */ }
    }
    void respond(Messenger to,int id,String result,long nanos) {
        // The early Stop signal deliberately has no reply waiter.
        if (to == null) return;
        Message m=Message.obtain(null,1); m.arg1=id;
        Bundle b=new Bundle(); b.putString("result",result); b.putLong("nanos",nanos); b.putInt("pid",Process.myPid()); b.putInt("uid",Process.myUid()); b.putInt("threads",Thread.activeCount());
        Debug.MemoryInfo mem=new Debug.MemoryInfo(); Debug.getMemoryInfo(mem); b.putInt("pss_kb",mem.getTotalPss()); b.putLong("java_heap",Runtime.getRuntime().totalMemory()-Runtime.getRuntime().freeMemory());
        m.setData(b);
        // Owner responses can contain keys, pairing authority and private content.
        // Diagnostics record lifecycle metadata only, never response bodies.
        try { to.send(m); } catch(RemoteException e) { evidence("reply_connection_lost",e.toString()); android.util.Log.i("JournalRuntime","reply connection lost: "+e); }
    }
    synchronized void evidence(String name,String detail) {
        if(Process.myUid()!=getApplicationInfo().uid)return;
        try(FileOutputStream f=new FileOutputStream(new File(getFilesDir(),"worker-evidence.jsonl"),true)) {
            JSONObject row=new JSONObject(); row.put("event",name);row.put("detail",detail);row.put("pid",Process.myPid());row.put("elapsed_ms",SystemClock.elapsedRealtime());
            f.write((row.toString()+"\n").getBytes(StandardCharsets.UTF_8));f.getFD().sync();
        }catch(Exception e){android.util.Log.e("JournalRuntime","worker evidence write failed",e);}
    }
    public IBinder onBind(Intent i) { return incoming.getBinder(); }
    public void onDestroy() {
        destroyed = true;
        ownerRequests.shutdown();
        evidence("service_destroyed","last binding released");
        execution.retire(this::cancelNativeWork, () -> Native.run("", "serve:{\"op\":\"stop\"}", 1));
        setupForeground.close();
        Native.retireSavedFolderProvider(savedFolderProviderIdentity);
        super.onDestroy();
    }
}
