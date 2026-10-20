// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import android.app.Activity;
import android.content.*;
import android.net.*;
import android.os.*;
import android.view.WindowManager;
import android.widget.*;
import org.json.*;
import java.io.*;
import java.net.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.concurrent.*;
import java.util.concurrent.atomic.AtomicLong;

/** Development owner controls. Journal requests arrive only at the SPL door. */
public class ServingActivity extends Activity {
    volatile Messenger worker;
    TextView status;
    FileObserver commands;
    int sequence;
    boolean bound;
    WorkerConnection connection;
    volatile boolean ownerStopped;
    volatile long readyConnection;
    final AtomicLong connectionGeneration = new AtomicLong();
    volatile boolean destroyed;
    String root;
    final Handler ui = new Handler(Looper.getMainLooper());
    final ExecutorService control = Executors.newSingleThreadExecutor();
    final ConcurrentHashMap<Integer, CompletableFuture<JSONObject>> pending = new ConcurrentHashMap<>();
    final Messenger replies = new Messenger(new Handler(Looper.getMainLooper()) {
        public void handleMessage(Message message) {
            CompletableFuture<JSONObject> future = pending.remove(message.arg1);
            if (future != null) {
                Bundle b = message.getData();
                JSONObject row = MainActivity.obj("native", MainActivity.parse(b.getString("result")),
                    "pid", b.getInt("pid"), "uid", b.getInt("uid"), "pss_kb", b.getInt("pss_kb"),
                    "java_heap", b.getLong("java_heap"), "native_ms", b.getLong("nanos") / 1e6);
                future.complete(row);
            }
        }
    });
    final class WorkerConnection implements ServiceConnection {
        public void onServiceConnected(ComponentName name, IBinder binder) {
            if (destroyed || connection != this) return;
            final long generation = connectionGeneration.incrementAndGet();
            final Messenger target = new Messenger(binder);
            worker = target;
            readyConnection = 0;
            if (ownerStopped) {
                control.execute(() -> {
                    if (currentConnection(generation, target)) {
                        readyConnection = generation;
                        consumeCommand();
                    }
                });
            } else {
                connectService(generation, target, false);
            }
        }
        public void onServiceDisconnected(ComponentName name) {
            connectionLost(this);
        }
        public void onBindingDied(ComponentName name) {
            connectionLost(this);
        }
    }
    void bindWorker() {
        if (destroyed || ownerStopped) return;
        connection = new WorkerConnection();
        bound = bindService(new Intent(this, JournalService.class), connection, Context.BIND_AUTO_CREATE);
        if (!bound) show(MainActivity.obj("error", "worker binding refused"));
    }
    void connectionLost(WorkerConnection lost) {
        if (destroyed || connection != lost) return;
        final long generation = connectionGeneration.incrementAndGet();
        readyConnection = 0;
        worker = null;
        for (CompletableFuture<JSONObject> future: pending.values()) future.completeExceptionally(new EOFException("worker lost"));
        pending.clear();
        show(MainActivity.obj("error", "worker disconnected"));
        // A retained auto-create binding need not promptly restart its worker.
        // Replace only this lost binding; a newer callback or Stop wins.
        ui.post(() -> {
            if (generation != connectionGeneration.get() || destroyed || ownerStopped
                    || connection != lost || worker != null) return;
            if (bound) unbindService(lost);
            bound = false;
            connection = null;
            bindWorker();
        });
    }
    boolean currentConnection(long generation, Messenger target) {
        return !destroyed && connectionGeneration.get() == generation && worker == target;
    }
    void connectService(final long generation, final Messenger target, final boolean startAlreadySucceeded) {
        control.execute(() -> {
            if (!currentConnection(generation, target) || ownerStopped) return;
            try {
                ServingConnection.Action action = ServingConnection.drive(generation, startAlreadySucceeded, new ServingConnection.Environment() {
                    public long currentGeneration() {
                        return ownerStopped ? -1 : connectionGeneration.get();
                    }
                    public ServingConnection.Retirement retirement() throws Exception {
                        JSONObject res = call(target, MainActivity.obj("op", "status"));
                        return ServingConnection.retirement(res, (row, key) ->
                            row instanceof JSONObject ? ((JSONObject)row).opt(key) : null);
                    }
                    public boolean start() throws Exception {
                        JSONObject res = call(target, MainActivity.obj("op", "start",
                            "address", wifiAddress() + ":7657",
                            "onnx_library", getApplicationInfo().nativeLibraryDir + "/libonnxruntime.so"));
                        show(res);
                        JSONObject nat = res.optJSONObject("native");
                        return nat != null && nat.optBoolean("ok", false);
                    }
                    public boolean enableAudio() throws Exception {
                        JSONObject res = call(target, MainActivity.obj("op", "audio-enable"));
                        show(res);
                        JSONObject nat = res.optJSONObject("native");
                        return nat != null && nat.optBoolean("ok", false);
                    }
                    public void onReady(long gen) {
                        readyConnection = gen;
                        consumeCommand();
                    }
                });
                if (action == ServingConnection.Action.RETRY) {
                    if (currentConnection(generation, target)) {
                        connectService(generation, target, startAlreadySucceeded);
                    }
                }
            } catch (Exception e) {
                show(MainActivity.obj("error", e.toString()));
            }
        });
    }
    String wifiAddress() throws Exception {
        ConnectivityManager cm = (ConnectivityManager)getSystemService(CONNECTIVITY_SERVICE);
        for (Network network: cm.getAllNetworks()) {
            NetworkCapabilities caps = cm.getNetworkCapabilities(network);
            LinkProperties links = cm.getLinkProperties(network);
            if (caps == null || links == null || !caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)) continue;
            for (LinkAddress link: links.getLinkAddresses()) {
                InetAddress address = link.getAddress();
                if (address instanceof Inet4Address && address.isSiteLocalAddress()) return address.getHostAddress();
            }
        }
        throw new IOException("private Wi-Fi IPv4 unavailable");
    }
    JSONObject call(JSONObject command) throws Exception {
        return call(worker, command);
    }
    JSONObject call(Messenger target, JSONObject command) throws Exception {
        if (target == null) throw new IOException("worker unavailable");
        int id = ++sequence;
        CompletableFuture<JSONObject> result = new CompletableFuture<>(); pending.put(id, result);
        Message message = Message.obtain(null, 1); message.arg1 = id; message.replyTo = replies;
        Bundle b = new Bundle(); b.putString("root", root); b.putString("op", "serve:"+command); b.putInt("count", 1); message.setData(b);
        try { target.send(message); return result.get(90, TimeUnit.SECONDS); }
        finally { pending.remove(id); }
    }
    void show(JSONObject row) { ui.post(()->status.setText(row.toString())); }
    void unpackPayload(String asset, File destination) throws IOException {
        PackagedPayload.unpack(new PackagedPayload.Assets() {
            public String[] list(String name) throws IOException { return getAssets().list(name); }
            public InputStream open(String name) throws IOException { return getAssets().open(name); }
        }, asset, destination);
    }
    // Debug USB automation writes this file through run-as, which has the app UID.
    // Intent extras and external IPC never carry owner requests.
    void consumeCommand() {
        // Keep an early USB command until Binder startup has completed.
        final long generation = connectionGeneration.get();
        final Messenger target = worker;
        if (readyConnection == 0 || readyConnection != generation
                || !currentConnection(generation, target) || target == null) return;
        File command = new File(getFilesDir(), "serving-command.json");
        if (!command.isFile()) return;
        String id = "unread";
        JSONObject row;
        try {
            JSONObject input = new JSONObject(new String(Files.readAllBytes(command.toPath()), StandardCharsets.UTF_8));
            id = input.getString("id");
            if (readyConnection != generation || !currentConnection(generation, target)) return;
            if ("stop".equals(input.optString("op"))) ownerStopped = true;
            if (!command.delete()) throw new IOException("command retirement failed");
            row = call(target, input);
        } catch(Exception e) { row = MainActivity.obj("error", e.toString()); }
        try {
            row.put("id", id); row.put("host_pid", android.os.Process.myPid());
            File stage = new File(getFilesDir(), "serving-reply.stage");
            try(FileOutputStream output = new FileOutputStream(stage)) {
                output.write(row.toString().getBytes(StandardCharsets.UTF_8)); output.getFD().sync();
            }
            Files.move(stage.toPath(), new File(getFilesDir(), "serving-reply.json").toPath(), java.nio.file.StandardCopyOption.REPLACE_EXISTING);
            show(row);
        } catch(Exception e) { android.util.Log.e("JournalServing", "reply write failed", e); }
    }
    public void onCreate(Bundle state) {
        super.onCreate(state); getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        root = new File(getFilesDir(), "synthetic-serving-journal").toString();
        new File(root).mkdirs();
        LinearLayout controls = new LinearLayout(this); controls.setOrientation(LinearLayout.VERTICAL);
        status = new TextView(this); status.setTextSize(16);
        for(String op: new String[]{"status", "index", "stop"}) {
            Button button = new Button(this); button.setText(op);
            button.setOnClickListener(v->{
                if ("stop".equals(op)) ownerStopped = true;
                final long gen = connectionGeneration.get();
                final Messenger target = worker;
                control.execute(()->{
                    if (ServingConnection.decide(gen, connectionGeneration.get(), true, true, true, ServingConnection.Retirement.STOPPED) == ServingConnection.Action.DROP) return;
                    if (!currentConnection(gen, target)) return;
                    try {
                        show(call(target, MainActivity.obj("op", op)));
                        if ("stop".equals(op) && currentConnection(gen, target)) {
                            readyConnection = gen;
                            consumeCommand();
                        }
                    }
                    catch(Exception e) { show(MainActivity.obj("error", e.toString())); }
                });
            });
            controls.addView(button);
        }
        controls.addView(status); setContentView(controls);
        commands = new FileObserver(getFilesDir(), FileObserver.MOVED_TO | FileObserver.CLOSE_WRITE) {
            public void onEvent(int event, String path) {
                if ("serving-command.json".equals(path)) control.execute(()->consumeCommand());
            }
        };
        commands.startWatching();
        try {
            unpackPayload("journal-payload", new File(getFilesDir(), "journal-payload"));
            bindWorker();
        } catch(IOException e) { show(MainActivity.obj("error", e.toString())); }
    }
    protected void onDestroy() {
        destroyed = true; connectionGeneration.incrementAndGet(); readyConnection = 0;
        commands.stopWatching(); if(bound) unbindService(connection); control.shutdownNow(); super.onDestroy();
    }
}
