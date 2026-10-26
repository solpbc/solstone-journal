// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import android.content.*;
import android.database.Cursor;
import android.net.Uri;
import android.net.*;
import android.os.*;
import android.provider.DocumentsContract;
import android.system.ErrnoException;
import android.system.Os;
import android.system.OsConstants;
import android.view.*;
import android.webkit.*;
import android.widget.*;
import org.json.*;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.util.*;
import java.util.concurrent.*;

/** Ordinary foreground owner controls around the existing Journal router. */
public final class OwnerActivity extends ServingActivity {
    WebView web;
    String boundAddress="";
    String runState="starting",reason="";
    boolean restarting;
    boolean runtimeUp;
    LinearLayout layout,bar,barActions;
    final java.util.concurrent.atomic.AtomicBoolean checking=new java.util.concurrent.atomic.AtomicBoolean();
    ConnectivityManager connectivity;
    ConnectivityManager.NetworkCallback networkCallback;
    boolean stopChoiceUnsaved;
    volatile boolean payloadReady,preparing;
    volatile OwnerTransport transport;
    volatile String pageToken = "";
    final ConcurrentHashMap<String,OwnerTransport.Cancellation> fetches = new ConcurrentHashMap<>();
    final ExecutorService requests = new ThreadPoolExecutor(4,4,0L,TimeUnit.MILLISECONDS,
        new ArrayBlockingQueue<Runnable>(32),new ThreadPoolExecutor.AbortPolicy());
    final ExecutorService uploads = new ThreadPoolExecutor(1,32,0L,TimeUnit.MILLISECONDS,
        new SynchronousQueue<Runnable>(),new ThreadPoolExecutor.AbortPolicy());
    ValueCallback<Uri[]> pendingChooser;
    final ConcurrentHashMap<String,StreamPump> pumps = new ConcurrentHashMap<>();
    final OwnerUploadBoard uploadBoard = new OwnerUploadBoard();
    final OwnerFolderPicker folderPicker = new OwnerFolderPicker();
    volatile int folderGeneration;
    boolean diagnosticCommands() { return false; }
    boolean bindWhileStopped() { return true; }
    void stoppedConnectionReady() {
        try { show(call(MainActivity.obj("op",ownerStopped ? "stop" : "status"))); }
        catch(Exception error) { show(MainActivity.obj("error","journal status unavailable")); }
    }
    void connectionRefused() { refreshStatus(); }
    boolean restoreOwnerStop() { return getSharedPreferences("owner",0).getBoolean("stopped",false); }
    boolean automaticStartAllowed() { return payloadReady && !ownerStopped && !restoreOwnerStop(); }
    void setOwnerStop(boolean stopped) {
        boolean saved=getSharedPreferences("owner",0).edit().putBoolean("stopped",stopped).commit();
        if (!saved && !stopped) {
            // A failed commit also changes SharedPreferences' in-memory map.
            // Keep the preceding stop choice in both views and refuse Start.
            getSharedPreferences("owner",0).edit().putBoolean("stopped",ownerStopped).commit();
            throw new IllegalStateException("couldn't save start · free some space, then try again");
        }
        ownerStopped=stopped; stopChoiceUnsaved=stopped && !saved;
    }
    String wifiAddress() {
        try { return super.wifiAddress(); } catch(Exception unavailable) { return "127.0.0.1"; }
    }
    boolean enableAudio(Messenger target) throws Exception {
        boolean enabled=super.enableAudio(target);
        if (!enabled) {
            payloadReady=false;
            ui.post(()->{
                if (destroyed || ownerStopped) return;
                runState="failed";reason="speech recognition couldn't start · reopen the app to check its files";renderBar();
            });
        }
        return enabled;
    }
    List<PackagedAudio.Pin> pins(JSONArray rows) throws JSONException {
        List<PackagedAudio.Pin> pins=new ArrayList<>();
        for(int i=0;i<rows.length();i++) {
            JSONObject row=rows.getJSONObject(i);
            pins.add(new PackagedAudio.Pin(row.getString("name"),row.getLong("bytes"),row.getString("sha256")));
        }
        return pins;
    }
    // Matches /init's decimal speech-download size formatter.
    static String size(long bytes) {
        if(bytes < 1000) return bytes+" B";
        String[] units={"KB","MB","GB","TB"}; int unit=0;double value=bytes/1000.0;
        while(value >= 1000 && unit < units.length-1) {value/=1000;unit++;}
        return (value == Math.floor(value) ? String.format(Locale.ROOT,"%.0f",value)
            : String.format(Locale.ROOT,"%.1f",Math.round(value*10)/10.0))+" "+units[unit];
    }
    void prepareAndBind() {
        if (preparing || destroyed) return;
        if (ownerStopped) { bindWorker();return; }
        preparing=true; runState="starting"; reason="preparing speech recognition…"; renderBar();
        control.execute(()->{
            try {
              synchronized(PackagedAudio.class) {
                if (destroyed || ownerStopped) return;
                unpackPayload("journal-payload",new File(getFilesDir(),"journal-payload"));
                if (destroyed || ownerStopped) return;
                JSONObject manifest;
                try(InputStream input=getAssets().open("audio-models/manifest.json")) {
                    manifest=new JSONObject(new String(OwnerTransport.readBounded(input,65536),StandardCharsets.UTF_8));
                }
                PackagedPayload.Assets assets=new PackagedPayload.Assets() {
                    public String[] list(String name) throws IOException { return getAssets().list(name); }
                    public InputStream open(String name) throws IOException { return getAssets().open(name); }
                };
                List<PackagedAudio.Pin> models=pins(manifest.getJSONArray("models")),notices=pins(manifest.getJSONArray("notices"));
                long total=0; for(PackagedAudio.Pin pin:models) total+=pin.bytes;for(PackagedAudio.Pin pin:notices) total+=pin.bytes;
                final long totalBytes=total;
                final long[] progress={0,0};
                PackagedAudio.prepare(assets,models,notices,
                    new File(getFilesDir(),"audio-models"),new File(getFilesDir(),"packaged-audio-staging"),
                    ()->!destroyed && !ownerStopped,()->ui.post(()->{
                        if (destroyed || ownerStopped) return;
                        payloadReady=true;
                        if (worker == null) bindWorker();
                        else connectService(connectionGeneration.get(),worker,false);
                    }),count->{
                        progress[0]+=count; long now=SystemClock.uptimeMillis();
                        if(now-progress[1] < 250 && progress[0] != totalBytes) return;
                        progress[1]=now;
                        final long done=progress[0];
                        ui.post(()->{
                            if(destroyed || ownerStopped || payloadReady) return;
                            reason="preparing speech recognition… "+size(done)+" / "+size(totalBytes);renderBar();
                        });
                    });
              }
            } catch(Exception error) {
                ui.post(()->{
                    if (destroyed) return;
                    if (!ownerStopped) {
                        runState="failed";
                        reason="speech recognition preparation stopped · free some space or check the installed files, then use start";
                    }
                    renderBar();
                });
            } finally { preparing=false; }
        });
    }
    void buildControls() {
        if (getActionBar() != null) getActionBar().hide();
        getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        layout=new LinearLayout(this); layout.setOrientation(LinearLayout.VERTICAL);
        layout.setFitsSystemWindows(true);
        bar=new LinearLayout(this); bar.setGravity(Gravity.CENTER_VERTICAL);
        status=new TextView(this); status.setTextSize(16); status.setPadding(16,8,16,8);
        barActions=new LinearLayout(this);
        bar.addView(status,new LinearLayout.LayoutParams(0,LinearLayout.LayoutParams.WRAP_CONTENT,1));
        bar.addView(barActions); layout.addView(bar);
        web=new WebView(this);
        WebSettings settings=web.getSettings(); settings.setJavaScriptEnabled(true); settings.setDomStorageEnabled(true);
        settings.setAllowFileAccess(false); settings.setAllowContentAccess(false);
        settings.setMixedContentMode(WebSettings.MIXED_CONTENT_NEVER_ALLOW);
        settings.setSafeBrowsingEnabled(false); settings.setCacheMode(WebSettings.LOAD_NO_CACHE);
        settings.setSupportMultipleWindows(false);
        CookieManager.getInstance().setAcceptThirdPartyCookies(web,false);
        try(InputStream input=getAssets().open("journal-web-host.json")) {
            JSONObject contract=new JSONObject(new String(OwnerTransport.readBounded(input,4096),StandardCharsets.UTF_8));
            settings.setUserAgentString(settings.getUserAgentString()+" "+contract.getString("user_agent_product"));
        } catch(Exception error) { throw new IllegalStateException("journal web contract unavailable",error); }
        web.addJavascriptInterface(new Bridge(),"JournalOwner");
        web.setWebChromeClient(new WebChromeClient() {
            public boolean onConsoleMessage(ConsoleMessage message) { return true; }
            public boolean onJsAlert(WebView view,String url,String message,JsResult result) {
                new android.app.AlertDialog.Builder(OwnerActivity.this).setMessage(message)
                    .setPositiveButton(android.R.string.ok,(dialog,which)->result.confirm())
                    .setOnCancelListener(dialog->result.cancel()).show();
                return true;
            }
            public boolean onJsConfirm(WebView view,String url,String message,JsResult result) {
                new android.app.AlertDialog.Builder(OwnerActivity.this).setMessage(message)
                    .setPositiveButton(android.R.string.ok,(dialog,which)->result.confirm())
                    .setNegativeButton(android.R.string.cancel,(dialog,which)->result.cancel())
                    .setOnCancelListener(dialog->result.cancel()).show();
                return true;
            }
            public boolean onShowFileChooser(WebView view,ValueCallback<Uri[]> callback,FileChooserParams params) {
                finishChooser(null);
                pendingChooser=callback;
                StringBuilder accept=new StringBuilder();
                String[] types=params == null ? null : params.getAcceptTypes();
                if (types != null) {
                    for (int index=0; index<types.length; index++) {
                        if (index > 0) accept.append(',');
                        if (types[index] != null) accept.append(types[index]);
                    }
                }
                Intent intent=new Intent(Intent.ACTION_GET_CONTENT).addCategory(Intent.CATEGORY_OPENABLE);
                intent.setType(OwnerAccept.mime(accept.toString()));
                try { startActivityForResult(intent,41); }
                catch(ActivityNotFoundException error) { finishChooser(null); }
                return true;
            }
        });
        web.setWebViewClient(new WebViewClient() {
            public boolean shouldOverrideUrlLoading(WebView view,WebResourceRequest request) {
                try { OwnerOrigin.admit(request.getUrl().toString()); return !request.isForMainFrame(); }
                catch(Exception external) {
                    if (request.isForMainFrame() && request.hasGesture()) openExternal(request.getUrl().toString());
                    // The existing native-host consent contract navigates after an async response.
                    else if (request.isForMainFrame()) openExternal(request.getUrl().toString());
                    return true;
                }
            }
            public WebResourceResponse shouldInterceptRequest(WebView view,WebResourceRequest request) {
                try { return resource(request); }
                catch(Exception error) {
                    if (request.isForMainFrame()) ui.post(()->pageError("couldn't open this page · open home or health to try again"));
                    return refused();
                }
            }
            public void onReceivedError(WebView view,WebResourceRequest request,WebResourceError error) {
                if (request.isForMainFrame()) pageError("couldn't open this page · open home or health to try again");
            }
            public void onReceivedHttpError(WebView view,WebResourceRequest request,WebResourceResponse response) {
                if (request.isForMainFrame()) pageError("this page is unavailable · check health before trying again");
            }
        });
        layout.addView(web,new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,0,1));
        setContentView(layout);
        // A saved choice is intent, not evidence that the process owner drained.
        runState=ownerStopped ? "stopunknown" : "starting";
        renderBar();
        connectivity=(ConnectivityManager)getSystemService(CONNECTIVITY_SERVICE);
        networkCallback=new ConnectivityManager.NetworkCallback() {
            public void onAvailable(Network network) { ui.post(()->renderBar()); }
            public void onLost(Network network) { ui.post(()->renderBar()); }
            public void onLinkPropertiesChanged(Network network,LinkProperties properties) { ui.post(()->renderBar()); }
        };
        connectivity.registerNetworkCallback(new NetworkRequest.Builder().addTransportType(NetworkCapabilities.TRANSPORT_WIFI).build(),networkCallback);
    }
    void pageError(String message) { reason=message; renderBar(); }
    void action(String label,Runnable callback,boolean primary) {
        Button button=primary ? new Button(this) : new Button(this,null,android.R.attr.borderlessButtonStyle);
        button.setText(label); button.setOnClickListener(v->callback.run()); barActions.addView(button);
    }
    void stopAction() {
        action("stop",()->new android.app.AlertDialog.Builder(this).setTitle("stop your journal?")
            .setMessage("until you start your journal again, your devices can't reach it and nothing new is processed.")
            .setPositiveButton("stop",(dialog,which)->stopJournal(false))
            .setNegativeButton("cancel",null).show(),false);
    }
    void renderBar() {
        if (destroyed || status == null) return;
        barActions.removeAllViews();
        String text;
        boolean centered="stopped".equals(runState) || "failed".equals(runState);
        layout.setGravity(centered ? Gravity.CENTER_VERTICAL : Gravity.TOP);
        web.setVisibility(centered || ownerStopped ? View.GONE : View.VISIBLE);
        switch(runState) {
            case "running":
                text="your journal is running";
                String wifi=wifiAddress();
                if ("127.0.0.1".equals(wifi)) text="no wifi · devices on your wifi can't reach your journal";
                else if (!boundAddress.equals(wifi+":7657")) {
                    text="wifi changed · restart so your devices can reach it";
                    action("restart",()->stopJournal(true),false);
                }
                stopAction();
                break;
            case "stopping": text="stopping your journal…\nfinishing current work · keep this app open";break;
            case "stopunknown":
                text="checking whether your journal has stopped…";
                action("check again",()->stopJournal(false),false);break;
            case "stopped":
                text="you stopped your journal";
                if (stopChoiceUnsaved) action("save stop",()->stopJournal(false),true);
                else action("start",()->ownerAction("start"),true);
                break;
            case "recovering": text="stopped unexpectedly · starting again…";stopAction();break;
            case "failed":
                text="your journal couldn't start";
                if(runtimeUp) stopAction(); else action("start",()->ownerAction("start"),true);
                break;
            case "stopfailed":
                text="couldn't finish stopping · keep this app open";
                action("check again",()->stopJournal(false),false);break;
            default: text="starting your journal…";stopAction();
        }
        if (!reason.isEmpty()) text+="\n"+reason;
        if (stopChoiceUnsaved) text+="\ncouldn't save stop · keep this app open";
        if ("stopping".equals(runState) || "stopunknown".equals(runState) || "stopfailed".equals(runState))
            getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        else getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        status.setText(text);
    }
    void refreshStatus() {
        if (destroyed || worker == null || !checking.compareAndSet(false,true)) return;
        final Messenger target=worker; final long generation=connectionGeneration.get();
        control.execute(()->{
            try { if(currentConnection(generation,target)) show(call(target,MainActivity.obj("op","status"))); }
            catch(Exception error) { if(currentConnection(generation,target)) show(MainActivity.obj("error","journal status unavailable")); }
            finally { checking.set(false); }
        });
    }
    protected void onResume() { super.onResume(); if(status != null) { renderBar();refreshStatus(); } }
    void openExternal(String value) {
        try {
            java.net.URI uri=java.net.URI.create(value);
            if (!("https".equals(uri.getScheme()) || "http".equals(uri.getScheme())) || uri.getHost()==null || uri.getRawUserInfo()!=null) return;
            startActivity(new Intent(Intent.ACTION_VIEW,Uri.parse(value)).addCategory(Intent.CATEGORY_BROWSABLE));
        } catch(Exception error) { pageError("couldn't open your browser"); }
    }
    void retireDocument() {
        folderPicker.retire();
        uploadBoard.retire();
        folderGeneration++;
        for(OwnerTransport.Cancellation cancellation:fetches.values()) cancellation.cancel();
        fetches.clear();
        for(StreamPump pump:pumps.values()) pump.cancellation.cancel();
        pumps.clear();
    }
    void closePage() {
        pageToken="";
        retireDocument();
        finishChooser(null);
        if (transport != null) { transport.close(); transport=null; }
        if (web != null) { web.stopLoading(); web.loadUrl("about:blank"); }
    }
    void runtimeReady() {
        ui.post(()->{
            if (destroyed || ownerStopped || readyConnection != connectionGeneration.get()) return;
            closePage();
            transport=new OwnerTransport(this); pageToken=UUID.randomUUID().toString();
            runState="running";reason="";renderBar();
            web.loadUrl(OwnerOrigin.ORIGIN+"/init");
        });
    }
    void connectionLost(WorkerConnection lost) {
        if (destroyed || connection != lost) return;
        final long generation=connectionGeneration.get();
        ui.post(()->{
            if (destroyed || connection != lost || connectionGeneration.get() != generation+1) return;
            closePage(); if(!ownerStopped) {runState="recovering";reason="";renderBar();}
        });
        super.connectionLost(lost);
    }
    void ownerAction(String action) {
        if ("start".equals(action)) {
            if (!ServingConnection.ownerStartAllowed(runState,preparing,runtimeUp,!stopChoiceUnsaved)) return;
            try { setOwnerStop(false); } catch(Exception error) { pageError(error.getMessage()); return; }
            runState="starting";reason="";renderBar();
            if (!payloadReady) { prepareAndBind();return; }
            if (worker == null) bindWorker();
            else connectService(connectionGeneration.get(),worker,false);
        }
    }
    void stopJournal(boolean restart) {
        lifecycle("owner_stop_requested");
        restarting=restart;
        ownerStopped=true;
        readyConnection=0;
        runState="stopping";reason="";renderBar();
        status.postOnAnimation(()->lifecycle("owner_stop_pending_frame_callback"));
        final Messenger target=worker; final long generation=connectionGeneration.get();
        ServingConnection.stop(control,()->requestStop(target),()->{
            if (!currentConnection(generation,target)) return;
            try { show(call(target,MainActivity.obj("op","stop"))); }
            catch(Exception error) { if(currentConnection(generation,target)) show(MainActivity.obj("error","journal status unavailable")); }
        });
        // Deliver Stop before closing response pipes can enqueue their cancels.
        closePage();
        setOwnerStop(true);
        if(target == null) bindWorker();
    }
    void show(JSONObject row) {
        ui.post(()->{
            if (destroyed || status == null) return;
            JSONObject nativeResult=row.optJSONObject("native");
            JSONObject value=nativeResult == null ? null : nativeResult.optJSONObject("value");
            if (value != null && value.opt("interface_address") instanceof String) {boundAddress=value.optString("interface_address");runtimeUp=true;}
            boolean stopped=value != null && (value.optBoolean("stopped") || value.has("stopping") && !value.optBoolean("stopping") && !value.optBoolean("failed"))
                || nativeResult != null && "server stopped".equals(nativeResult.optString("error"));
            if(stopped) runtimeUp=false;
            if (value != null && value.optBoolean("failed")) { runState="stopfailed";restarting=false; }
            else if (value != null && value.has("root_current") && !value.optBoolean("root_current")) {
                runState="stopfailed";reason="journal storage unavailable";restarting=false;
            } else if (value != null && value.optBoolean("stopping")) {
                runState="stopping";reason="";
                ui.postDelayed(this::refreshStatus,1000);
            } else if (stopped && ownerStopped) {
                runState="stopped";reason="";
                lifecycle("owner_stopped_observed");
                if (restarting && !stopChoiceUnsaved) { restarting=false;ownerAction("start");return; }
            } else if (ownerStopped) {
                runState="stopunknown";
                reason="keep this app open · use check again to confirm";
                ui.postDelayed(this::refreshStatus,1000);
            } else if ("worker disconnected".equals(row.optString("error"))) {
                runState="recovering";reason="";
            } else if (nativeResult == null || !nativeResult.optBoolean("ok")) {
                if (!preparing) {
                    runState="failed";
                    reason="worker disconnected".equals(row.optString("error")) ? "journal process disconnected" :
                        "journal controls unavailable · use start to try again";
                }
            }
            renderBar();
        });
    }
    static WebResourceResponse refused() {
        return new WebResourceResponse("text/plain","UTF-8",503,"Unavailable",Collections.emptyMap(),
            new ByteArrayInputStream("journal controls unavailable".getBytes(StandardCharsets.UTF_8)));
    }
    WebResourceResponse resource(WebResourceRequest request) throws Exception {
        String url=request.getUrl().toString(); String path=OwnerOrigin.path(url);
        if (!"GET".equals(request.getMethod()) && !"HEAD".equals(request.getMethod())) return refused();
        if ("/__android/owner.js".equals(path)) return new WebResourceResponse("application/javascript","UTF-8",getAssets().open("owner.js"));
        OwnerTransport current=transport;
        if (current == null) return refused();
        JSONArray headers=new JSONArray();
        for(Map.Entry<String,String> header:request.getRequestHeaders().entrySet()) {
            if (Arrays.asList("accept","range","if-none-match","if-modified-since","last-event-id").contains(header.getKey().toLowerCase(Locale.ROOT)))
                headers.put(new JSONArray().put(header.getKey()).put(header.getValue()));
        }
        OwnerTransport.Stream stream=current.follow(request.getMethod(),url,"",headers);
        String contentType=stream.headers.getOrDefault("content-type","application/octet-stream");
        String mime=contentType.split(";",2)[0];
        Map<String,String> responseHeaders=new TreeMap<>(String.CASE_INSENSITIVE_ORDER);
        responseHeaders.putAll(stream.headers);
        InputStream body=stream;
        if (request.isForMainFrame() && mime.equals("text/html")) {
            byte[] bytes;
            try { bytes=OwnerTransport.readBounded(stream,4*1024*1024); } finally { stream.close(); }
            String html=new String(bytes,StandardCharsets.UTF_8);
            // A new document must not receive a prior page's reply or folder result.
            String token=UUID.randomUUID().toString();
            pageToken=token;
            retireDocument();
            current.close();
            transport=new OwnerTransport(this);
            String injection="<script>window.__journalOwnerToken="+JSONObject.quote(token)+";history.replaceState(null,'',"+JSONObject.quote(stream.url)+");</script><script src=\"/__android/owner.js\"></script>";
            int head=html.indexOf("<head>");
            if (head < 0) throw new IOException("journal page has no head");
            html=html.substring(0,head+6)+injection+html.substring(head+6);
            responseHeaders.remove("content-length");
            responseHeaders.remove("etag"); responseHeaders.remove("last-modified");
            responseHeaders.put("cache-control","no-store");
            body=new ByteArrayInputStream(html.getBytes(StandardCharsets.UTF_8));
        }
        String policy="default-src 'self' data: blob:; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; connect-src 'self'; frame-src 'none'; object-src 'none'; base-uri 'self'";
        String existing=responseHeaders.get("content-security-policy");
        responseHeaders.put("Content-Security-Policy",existing == null ? policy : existing+", "+policy);
        if (stream.status < 200 || stream.status >= 300 && stream.status < 400) { body.close(); return refused(); }
        return new WebResourceResponse(mime,"UTF-8",stream.status,"Journal",responseHeaders,body);
    }
    final class Bridge {
        @JavascriptInterface public void cancel(String token,String id) {
            if (token == null || id == null || !token.equals(pageToken) || id.length() >= 64) return;
            String key=token+":"+id;
            OwnerTransport.Cancellation cancellation=fetches.remove(key);
            if (cancellation != null) cancellation.cancel();
            uploadBoard.cancel(key);
            StreamPump pump=pumps.remove(key);
            if (pump != null) pump.cancellation.cancel();
        }
        @JavascriptInterface public void fetch(String encoded) {
            if (encoded.length() > 384*1024) return;
            try {
                JSONObject request=new JSONObject(encoded);
                final String token=request.getString("token"), id=request.getString("id");
                if (!token.equals(pageToken) || token.isEmpty() || id.length() >= 64) return;
                final OwnerTransport current=transport;
                if (current == null) return;
                final OwnerTransport.Cancellation cancellation=current.new Cancellation();
                final String key=token+":"+id;
                if (fetches.putIfAbsent(key,cancellation) != null) return;
                try { requests.execute(()->{
                    JSONObject result;
                    try {
                        if (!token.equals(pageToken)) throw new IOException("journal request cancelled");
                        cancellation.check();
                        OwnerTransport.Stream stream=current.follow(request.getString("method"),request.getString("url"),request.optString("body",""),request.getJSONArray("headers"),cancellation);
                        byte[] bytes;
                        try { bytes=OwnerTransport.readBounded(stream,8*1024*1024); } finally { stream.close(); }
                        result=MainActivity.obj("status",stream.status,"headers",new JSONObject(stream.headers),"url",stream.url,
                            "body",android.util.Base64.encodeToString(bytes,android.util.Base64.NO_WRAP));
                    } catch(Exception error) { result=MainActivity.obj("error","journal response unavailable; check the result before trying again"); }
                    fetches.remove(key,cancellation);
                    final JSONObject reply=result;
                    ui.post(()->{
                        if (destroyed || !token.equals(pageToken) || cancellation.cancelled) return;
                        web.evaluateJavascript("window.__journalOwnerReply("+JSONObject.quote(token)+","+JSONObject.quote(id)+","+reply+")",null);
                    });
                }); } catch(RejectedExecutionException error) {
                    fetches.remove(key,cancellation);
                    ui.post(()->{
                        if (!destroyed && token.equals(pageToken)) web.evaluateJavascript("window.__journalOwnerReply("+
                            JSONObject.quote(token)+","+JSONObject.quote(id)+",{\"error\":\"journal controls busy; check health before retrying\"})",null);
                    });
                }
            } catch(Exception error) { /* No unbounded queue or diagnostic body recording. */ }
        }
        @JavascriptInterface public String openStream(String encoded) {
            if (encoded == null || encoded.length() > 384*1024) return "{\"ok\":false,\"error\":\"journal request too large\"}";
            try {
                JSONObject request=new JSONObject(encoded);
                final String token=request.getString("token"), id=request.getString("id");
                if (!token.equals(pageToken) || token.isEmpty() || id.length() >= 64) return "{\"ok\":false,\"error\":\"journal request cancelled\"}";
                final OwnerTransport current=transport;
                if (current == null) return "{\"ok\":false,\"error\":\"journal response unavailable; check the result before trying again\"}";
                final OwnerTransport.Cancellation cancellation=current.new Cancellation();
                final String key=token+":"+id;
                if (fetches.putIfAbsent(key,cancellation) != null) return "{\"ok\":false,\"error\":\"journal controls busy; check health before retrying\"}";
                if (!uploadBoard.admit(key)) {
                    fetches.remove(key,cancellation);
                    return "{\"ok\":false,\"error\":\"journal controls busy; check health before retrying\"}";
                }
                if (!token.equals(pageToken)) {
                    uploadBoard.cancel(key);
                    fetches.remove(key,cancellation);
                    return "{\"ok\":false,\"error\":\"journal request cancelled\"}";
                }
                try {
                    OwnerTransport.Upload upload=current.openStream(request.getString("method"),request.getString("url"),request.getJSONArray("headers"),cancellation);
                    if (!token.equals(pageToken)) {
                        cancellation.cancel();
                        uploadBoard.cancel(key);
                        fetches.remove(key,cancellation);
                        try { upload.bodyWrite.close(); } catch(IOException ignored) {}
                        try { upload.responseRead.close(); } catch(IOException ignored) {}
                        return "{\"ok\":false,\"error\":\"journal request cancelled\"}";
                    }
                    final StreamPump pump=new StreamPump(token,id,current,cancellation,upload,request.getString("url"));
                    pumps.put(key,pump);
                    try { uploads.execute(pump); }
                    catch(RejectedExecutionException busy) {
                        cancellation.cancel();
                        uploadBoard.cancel(key);
                        fetches.remove(key,cancellation);
                        pumps.remove(key,pump);
                        try { upload.bodyWrite.close(); } catch(IOException ignored) {}
                        try { upload.responseRead.close(); } catch(IOException ignored) {}
                        return "{\"ok\":false,\"error\":\"journal controls busy; check health before retrying\"}";
                    }
                    return "{\"ok\":true}";
                } catch(Exception error) {
                    uploadBoard.cancel(key);
                    fetches.remove(key,cancellation);
                    return "{\"ok\":false,\"error\":"+JSONObject.quote(ownerMessage(error))+"}";
                }
            } catch(Exception error) {
                return "{\"ok\":false,\"error\":"+JSONObject.quote(ownerMessage(error))+"}";
            }
        }
        @JavascriptInterface public String writeChunk(String token,String id,String chunk) {
            if (token == null || !token.equals(pageToken) || id == null) return "{\"error\":\"journal request cancelled\"}";
            StreamPump pump=pumps.get(token+":"+id);
            if (pump == null || pump.cancellation.cancelled) return "{\"error\":\"aborted; check the result before retrying\"}";
            try {
                byte[] bytes=OwnerBytes.latin1(chunk);
                if (!pump.chunks.offer(bytes)) return "{\"busy\":true}";
                return "{\"accepted\":true}";
            } catch(IllegalArgumentException error) {
                return "{\"error\":\"this file couldn't be sent. choose it again, and check the result before retrying.\"}";
            }
        }
        @JavascriptInterface public String finishStream(String token,String id) {
            if (token == null || !token.equals(pageToken) || id == null) return "{\"error\":\"journal request cancelled\"}";
            String key=token+":"+id;
            StreamPump pump=pumps.get(key);
            if (pump == null) return "{\"error\":\"journal response unavailable; check the result before trying again\"}";
            pump.finished=true;
            OwnerUploadBoard.Taken taken=uploadBoard.take(key);
            if (taken.pending) return "{\"pending\":true}";
            pumps.remove(key,pump);
            fetches.remove(key,pump.cancellation);
            if (taken.reply == null) return "{\"error\":\"journal response unavailable; check the result before trying again\"}";
            return taken.reply;
        }
        @JavascriptInterface public void chooseFolder(String token) {
            if (folderPicker.present(token, pageToken, folderGeneration) == null) return;
            ui.post(()->{
                if (!folderPicker.claimStart(pageToken, folderGeneration)) return;
                startFolderPicker();
            });
        }
    }
    protected void onActivityResult(int request,int result,Intent data) {
        if (request == 41) {
            Uri uri=data == null ? null : data.getData();
            if (result != RESULT_OK || uri == null) { finishChooser(null); return; }
            try {
                InputStream input=getContentResolver().openInputStream(uri);
                if (input == null) throw new IOException("couldn't be read");
                input.close();
                ValueCallback<Uri[]> callback=pendingChooser;
                pendingChooser=null;
                if (callback != null) callback.onReceiveValue(new Uri[]{uri});
            } catch(Exception error) {
                finishChooser(null);
                notice("this file couldn't be read. choose it again.");
            }
            return;
        }
        if (request == 42) {
            final Uri tree=data == null ? null : data.getData();
            boolean ok=result == RESULT_OK && tree != null;
            OwnerFolderPicker.Authority origin=folderPicker.acceptResult(ok, pageToken, folderGeneration);
            if (origin == null || tree == null) return;
            final int generation=origin.generation;
            final String token=origin.token;
            try { uploads.execute(()->copyFolder(tree,generation,token)); }
            catch(RejectedExecutionException busy) { notice("journal controls busy; check health before retrying"); }
        }
    }
    void finishChooser(Uri[] value) {
        ValueCallback<Uri[]> callback=pendingChooser;
        pendingChooser=null;
        if (callback != null) callback.onReceiveValue(value);
    }
    void startFolderPicker() {
        try { startActivityForResult(new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE),42); }
        catch(ActivityNotFoundException error) {
            folderPicker.acceptResult(false, pageToken, folderGeneration);
            notice("this file couldn't be read. choose it again.");
        }
    }
    void notice(String message) {
        ui.post(()->{ if (web != null && !destroyed) web.evaluateJavascript("alert("+JSONObject.quote(message)+")",null); });
    }
    void copyFolder(Uri tree,int generation,String token) {
        File cache=getCacheDir();
        OwnerBridgeFiles.Claim claim=null;
        try {
            if (!OwnerFolderPicker.same(token, generation, pageToken, folderGeneration)) return;
            claim=OwnerBridgeFiles.allocate(cache);
            File dest=OwnerBridgeFiles.directory(cache,claim.id);
            if (!OwnerFolderPicker.same(token, generation, pageToken, folderGeneration) || copyChildren(tree,DocumentsContract.getTreeDocumentId(tree),dest,generation,token)) {
                OwnerBridgeFiles.release(cache,claim);
                claim=null;
                return;
            }
            OwnerBridgeFiles.commit(cache,claim);
            final OwnerBridgeFiles.Claim owned=claim;
            claim=null;
            final String path=dest.getAbsolutePath();
            ui.post(()->{
                if (web == null || !OwnerFolderPicker.same(token, generation, pageToken, folderGeneration)) { OwnerBridgeFiles.release(cache,owned); return; }
                web.evaluateJavascript("(function(){var input=document.getElementById('guidedPathInput');if(!input)return;input.value="+JSONObject.quote(path)+";input.dispatchEvent(new Event('input',{bubbles:true}));})()",null);
                OwnerBridgeFiles.disarm(cache,owned);
            });
        } catch(Exception error) {
            if (claim != null) OwnerBridgeFiles.release(cache,claim);
            String message=error.getMessage() == null ? "" : error.getMessage();
            notice(message.contains("No space") || message.contains("ENOSPC")
                ? "your journal couldn't store this file. free some space and try again."
                : "this file couldn't be read. choose it again.");
        }
    }
    boolean copyChildren(Uri tree,String documentId,File dest,int generation,String token) throws IOException {
        Uri children=DocumentsContract.buildChildDocumentsUriUsingTree(tree,documentId);
        Cursor cursor=getContentResolver().query(children,new String[]{
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE},null,null,null);
        if (cursor == null) throw new IOException("couldn't be read");
        try {
            while (cursor.moveToNext()) {
                if (!OwnerFolderPicker.same(token, generation, pageToken, folderGeneration)) return true;
                String name=cursor.getString(1);
                if (name == null || name.length() == 0 || name.indexOf('/') >= 0 || ".".equals(name) || "..".equals(name)) continue;
                File child=new File(dest,name);
                String childId=cursor.getString(0);
                if (DocumentsContract.Document.MIME_TYPE_DIR.equals(cursor.getString(2))) {
                    if (!child.isDirectory() && !child.mkdirs()) throw new IOException("couldn't be read");
                    if (copyChildren(tree,childId,child,generation,token)) return true;
                } else {
                    InputStream input=getContentResolver().openInputStream(DocumentsContract.buildDocumentUriUsingTree(tree,childId));
                    if (input == null) throw new IOException("couldn't be read");
                    try { copyStream(input,child); } finally { input.close(); }
                }
            }
        } finally { cursor.close(); }
        return false;
    }
    void copyStream(InputStream input,File dest) throws IOException {
        byte[] buffer=new byte[64*1024];
        OutputStream output=new FileOutputStream(dest);
        try {
            int count;
            while ((count=input.read(buffer)) != -1) output.write(buffer,0,count);
        } catch(IOException error) {
            String message=error.getMessage() == null ? "" : error.getMessage();
            if (message.contains("ENOSPC") || message.contains("No space")) throw new IOException("No space");
            throw error;
        } finally { output.close(); }
    }
    static String ownerMessage(Exception error) {
        String message=error.getMessage() == null ? "" : error.getMessage();
        if (message.contains("transfer stopped")) return "the transfer stopped before it finished. check the result before trying again.";
        if (message.contains("No space") || message.contains("ENOSPC")) return "your journal couldn't store this file. free some space and try again.";
        if (message.contains("couldn't be read")) return "this file couldn't be read. choose it again.";
        if (message.contains("busy")) return "journal controls busy; check health before retrying";
        if (message.contains("cancelled") || message.contains("aborted")) return "aborted; check the result before retrying";
        return "journal response unavailable; check the result before trying again";
    }
    final class StreamPump implements Runnable {
        final String token,pageId,url;
        final OwnerTransport transport;
        final OwnerTransport.Cancellation cancellation;
        final OwnerTransport.Upload upload;
        final ArrayBlockingQueue<byte[]> chunks=new ArrayBlockingQueue<byte[]>(1);
        volatile boolean finished;
        StreamPump(String token,String pageId,OwnerTransport transport,OwnerTransport.Cancellation cancellation,OwnerTransport.Upload upload,String url) {
            this.token=token; this.pageId=pageId; this.transport=transport; this.cancellation=cancellation; this.upload=upload; this.url=url;
        }
        public void run() {
            try {
                Os.fcntlInt(upload.bodyWrite.getFileDescriptor(),OsConstants.F_SETFL,OsConstants.O_NONBLOCK);
                while (!finished || !chunks.isEmpty()) {
                    if (cancellation.cancelled || !token.equals(pageToken)) throw new IOException("cancelled");
                    byte[] chunk=chunks.poll(50,TimeUnit.MILLISECONDS);
                    if (chunk == null) continue;
                    int offset=0;
                    while (offset < chunk.length) {
                        if (cancellation.cancelled || !token.equals(pageToken)) throw new IOException("cancelled");
                        try {
                            int wrote=Os.write(upload.bodyWrite.getFileDescriptor(),java.nio.ByteBuffer.wrap(chunk,offset,chunk.length-offset));
                            if (wrote > 0) offset += wrote;
                        } catch(ErrnoException error) {
                            if (error.errno == OsConstants.EAGAIN) { Thread.sleep(50); continue; }
                            throw new IOException("couldn't be read");
                        }
                    }
                }
                upload.bodyWrite.close();
                long closedAt=SystemClock.elapsedRealtime();
                while (true) {
                    if (cancellation.cancelled || !token.equals(pageToken)) throw new IOException("cancelled");
                    JSONObject value=transport.poll(upload.id);
                    if (value.optBoolean("pending")) {
                        if (OwnerTransferWait.stalled(true, SystemClock.elapsedRealtime()-closedAt)) throw new IOException("transfer stopped");
                        Thread.sleep(50);
                        continue;
                    }
                    JSONObject headers=new JSONObject();
                    JSONArray pairs=value.getJSONArray("headers");
                    for (int index=0; index<pairs.length(); index++) {
                        JSONArray pair=pairs.getJSONArray(index);
                        headers.put(pair.getString(0),pair.getString(1));
                    }
                    byte[] body=OwnerTransport.readBounded(new ParcelFileDescriptor.AutoCloseInputStream(upload.responseRead),8*1024*1024);
                    uploadBoard.keep(token+":"+pageId,MainActivity.obj("status",value.getInt("status"),"headers",headers,"url",url,
                        "body",android.util.Base64.encodeToString(body,android.util.Base64.NO_WRAP)).toString());
                    return;
                }
            } catch(Exception error) {
                uploadBoard.keep(token+":"+pageId,"{\"error\":"+JSONObject.quote(ownerMessage(error))+"}");
                cancellation.cancel();
            } finally {
                try { upload.bodyWrite.close(); } catch(IOException ignored) {}
                try { upload.responseRead.close(); } catch(IOException ignored) {}
            }
        }
    }
    protected void onDestroy() {
        if (connectivity != null && networkCallback != null) connectivity.unregisterNetworkCallback(networkCallback);
        closePage(); requests.shutdownNow(); uploads.shutdownNow(); web.removeJavascriptInterface("JournalOwner"); web.destroy();
        super.onDestroy();
    }
    public void onBackPressed() {
        if (web.canGoBack()) web.goBack(); else super.onBackPressed();
    }
}
