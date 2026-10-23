// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import android.content.*;
import android.net.Uri;
import android.net.*;
import android.os.*;
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
        runState=ownerStopped ? "stopped" : "starting";
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
            .setMessage(("running".equals(runState) ? "work in progress finishes first. " : "")+
                "until you start your journal again, your devices can't reach it and nothing new is processed.")
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
            case "stopping": text="stopping · finishing work in progress";break;
            case "stopped": text="you stopped your journal";action("start",()->ownerAction("start"),true);break;
            case "recovering": text="stopped unexpectedly · starting again…";stopAction();break;
            case "failed":
                text="your journal couldn't start";
                if(runtimeUp) stopAction(); else action("start",()->ownerAction("start"),true);
                break;
            case "stopfailed": text="couldn't finish stopping · keep this app open";break;
            default: text="starting your journal…";stopAction();
        }
        if (!reason.isEmpty()) text+="\n"+reason;
        if (stopChoiceUnsaved) text+="\nstop choice couldn't be saved · check it again after reopening";
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
    void closePage() {
        pageToken="";
        for(OwnerTransport.Cancellation cancellation:fetches.values()) cancellation.cancel();
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
            if (preparing || "running".equals(runState)) return;
            try { setOwnerStop(false); } catch(Exception error) { pageError(error.getMessage()); return; }
            runState="starting";reason="";renderBar();
            if (!payloadReady) { prepareAndBind();return; }
            if (worker == null) bindWorker();
            else connectService(connectionGeneration.get(),worker,false);
        }
    }
    void stopJournal(boolean restart) {
        restarting=restart;
        setOwnerStop(true);
        readyConnection=0;
        closePage(); runState="stopping";reason="";renderBar();
        final Messenger target=worker; final long generation=connectionGeneration.get();
        if(target == null) { bindWorker();return; }
        control.execute(()->{
            if (!currentConnection(generation,target)) return;
            try { show(call(target,MainActivity.obj("op","stop"))); }
            catch(Exception error) { show(MainActivity.obj("error","journal status unavailable")); }
        });
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
                runState="stopping";
                ui.postDelayed(this::refreshStatus,1000);
            } else if (stopped && ownerStopped) {
                runState="stopped";reason="";
                if (restarting) { restarting=false;ownerAction("start");return; }
            } else if (ownerStopped) {
                runState="stopping";
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
            // A new document must never receive a prior page's asynchronous reply.
            current.close();
            transport=new OwnerTransport(this);
            String token=UUID.randomUUID().toString(); pageToken=token;
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
            if (token.equals(pageToken) && id.length() < 64) {
                OwnerTransport.Cancellation cancellation=fetches.get(token+":"+id);
                if (cancellation != null) cancellation.cancel();
            }
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
    }
    protected void onDestroy() {
        if (connectivity != null && networkCallback != null) connectivity.unregisterNetworkCallback(networkCallback);
        closePage(); requests.shutdownNow(); web.removeJavascriptInterface("JournalOwner"); web.destroy();
        super.onDestroy();
    }
    public void onBackPressed() {
        if (web.canGoBack()) web.goBack(); else super.onBackPressed();
    }
}
