// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.concurrent.atomic.AtomicLong;

/** The same acquisition, dispatch, retirement and Stop callbacks used by the adapters. */
public final class SetupBrowserTest {
    static void require(boolean value) { if (!value) throw new AssertionError(); }
    static final String URL="https://services.solstone.app/enable/spp?opaque=unchanged";
    static final class Fixture implements AutoCloseable {
        final AtomicLong now=new AtomicLong();
        final AtomicLong uptime=new AtomicLong();
        final SetupForeground foreground=new SetupForeground(now::get);
        final SetupBrowser browser=new SetupBrowser();
        final JournalExecution execution=new JournalExecution();
        final ArrayDeque<Runnable> service=new ArrayDeque<>(), replies=new ArrayDeque<>();
        final List<String> effects=new ArrayList<>();
        boolean connected=true, permission=true, promoted=true, launchFails;
        int launches, refusals, starts, finishes;
        long nextCheckUptime;
        Runnable alarm;
        String lastId;
        Fixture() { execution.admission.opened(0); browser.resume(); }
        final SetupBrowser.Connection connection=new SetupBrowser.Connection() {
            public boolean current() { return connected && execution.admission.isOpen(); }
            public void acquire(String id) {
                lastId=id;
                service.add(()->{
                    boolean acquired=foreground.begin(id,execution.admission.isOpen(),new SetupForeground.Effects() {
                        public boolean start() throws Exception {
                            nextCheckUptime=uptime.get()+SetupForeground.CHECK_INTERVAL_MS;
                            alarm=()->{
                                long delay=foreground.checkTimeout(id);
                                if (delay > 0) nextCheckUptime=uptime.get()+delay;
                            };
                            starts++; effects.add("foreground");
                            if (!permission) throw new Exception("platform refusal");
                            return promoted;
                        }
                        public void finish() { alarm=null; finishes++; effects.add("retired"); }
                    });
                    replies.add(()->browser.acknowledged(id,acquired));
                });
            }
            public void release(String id) { service.add(()->foreground.end(id)); }
            public void launch(String url) throws Exception {
                require(URL.equals(url));
                if (launchFails) throw new Exception("no browser");
                launches++; effects.add("browser"); browser.pause();
            }
            public void refused() { refusals++; }
        };
        void open() { browser.begin(connection,URL); }
        void drain() { while (!service.isEmpty()) service.remove().run(); }
        void reply() { while (!replies.isEmpty()) replies.remove().run(); }
        void advance(long elapsed, long awake) {
            now.addAndGet(elapsed); uptime.addAndGet(awake);
            if (alarm != null && uptime.get() >= nextCheckUptime) alarm.run();
        }
        void nativeStop() {
            foreground.stop(()->execution.beginStop(()->effects.add("native cancel")));
        }
        public void close() {
            browser.close(); drain(); foreground.close();
            execution.work.shutdown(); execution.thinking.shutdown();
        }
    }
    public static void main(String[] args) {
        require(SetupBrowser.protectedUrl(URL));
        require(SetupBrowser.protectedUrl("https://services.solstone.app:443/subscribe/spl"));
        for (String other : Arrays.asList("http://services.solstone.app/enable/spp",
                "https://services.solstone.app.evil/", "https://evil@services.solstone.app/",
                "https://services.solstone.app:444/", "https://solstone.app/", "file:///tmp/x"))
            require(!SetupBrowser.protectedUrl(other));

        try (Fixture f=new Fixture()) {
            f.open(); require(f.launches == 0); f.drain(); require(f.launches == 0);
            f.reply(); require(f.launches == 1 && f.finishes == 0);
            require(f.effects.equals(Arrays.asList("foreground","browser")));
            // Its own pause retains protection; duplicate ACK cannot dispatch again.
            f.browser.acknowledged(f.lastId,true); require(f.launches == 1);
            f.browser.resume(); f.drain(); require(f.finishes == 1);
            f.browser.close(); f.drain(); require(f.finishes == 1);
        }
        try (Fixture f=new Fixture()) {
            // Home before acquisition/ACK: even a late positive ACK opens nothing.
            f.open(); f.browser.pause(); f.drain(); f.reply();
            require(f.launches == 0 && f.starts == 1 && f.finishes == 1);
            f.browser.resume(); require(f.launches == 0);
            f.open(); f.drain(); f.reply(); require(f.launches == 1);
        }
        try (Fixture f=new Fixture()) {
            f.permission=false; f.open(); f.drain(); f.reply(); f.drain();
            require(f.launches == 0 && f.refusals == 1 && f.finishes == 1);
        }
        try (Fixture f=new Fixture()) {
            // No exception from the platform, but actual promotion was ignored.
            f.promoted=false; f.open(); f.drain(); f.reply(); f.drain();
            require(f.launches == 0 && f.refusals == 1 && f.finishes == 1);
        }
        try (Fixture f=new Fixture()) {
            f.launchFails=true; f.open(); f.drain(); f.reply(); f.drain();
            require(f.launches == 0 && f.refusals == 1 && f.finishes == 1);
        }
        try (Fixture f=new Fixture()) {
            f.open(); String abandoned=f.lastId; f.drain();
            // Lost reply expires locally and releases its exact remote acquisition.
            f.browser.acquisitionExpired(abandoned); f.drain();
            require(f.finishes == 1 && f.refusals == 1);
            f.reply(); require(f.launches == 0);
            f.open(); f.drain(); f.reply(); require(f.launches == 1);
            f.foreground.end(abandoned); require(f.finishes == 1);
        }
        try (Fixture f=new Fixture()) {
            f.open(); String old=f.lastId; f.drain();
            f.open(); f.drain(); f.reply();
            require(f.starts == 2 && f.finishes == 1 && f.launches == 1);
            f.foreground.end(old); require(f.finishes == 1);
            f.now.set(SetupForeground.BUDGET_MS); f.foreground.expire();
            require(f.finishes == 2);
        }
        try (Fixture f=new Fixture()) {
            // Sleep consumes the elapsed budget. On wake, the pending Handler
            // callback checks that deadline without owing a full uptime budget.
            f.open(); f.drain(); f.reply();
            f.advance(60000,60000); require(f.finishes == 0);
            f.advance(SetupForeground.BUDGET_MS-60000,0);
            require(f.now.get() == SetupForeground.BUDGET_MS && f.uptime.get() == 60000);
            require(f.finishes == 0);
            f.advance(SetupForeground.CHECK_INTERVAL_MS,SetupForeground.CHECK_INTERVAL_MS);
            require(f.finishes == 1 && f.alarm == null);
        }
        try (Fixture f=new Fixture()) {
            // The captured connection can change without an explicit close callback.
            f.open(); f.drain(); f.connected=false; f.reply(); f.drain();
            require(f.launches == 0 && f.finishes == 1 && f.refusals == 1);
        }
        try (Fixture f=new Fixture()) {
            // A resumed/recreated caller retires a pending open before its late ACK.
            f.open(); f.drain(); f.browser.resume(); f.drain(); f.reply();
            require(f.launches == 0 && f.finishes == 1);
        }
        try (Fixture f=new Fixture()) {
            // An old timer must not retire a successor with a later deadline.
            f.open(); String old=f.lastId; f.drain();
            f.now.set(1000); f.open(); f.drain(); f.reply();
            f.now.set(SetupForeground.BUDGET_MS);
            require(f.foreground.checkTimeout(old) == 0);
            f.foreground.end(old);
            require(f.finishes == 1 && f.launches == 1);
            f.now.set(SetupForeground.BUDGET_MS+999);
            require(f.foreground.checkTimeout(f.lastId) == 1 && f.finishes == 1);
            f.now.set(SetupForeground.BUDGET_MS+1000); f.foreground.expire();
            require(f.finishes == 2);
        }
        try (Fixture f=new Fixture()) {
            f.open(); f.drain(); f.connected=false;
            f.browser.close(); f.drain(); f.reply();
            require(f.launches == 0 && f.finishes == 1);
        }
        // Both early owner-stop and ordinary Stop use this same cancellation-first callback.
        for (boolean early : new boolean[]{true,false}) try (Fixture f=new Fixture()) {
            f.open(); f.drain();
            if (early) f.nativeStop(); else f.service.add(f::nativeStop);
            f.drain(); f.reply(); f.drain();
            require(f.launches == 0 && f.finishes == 1 && !f.execution.admission.isOpen());
            require(f.effects.indexOf("native cancel") < f.effects.indexOf("retired"));
            f.open(); f.drain(); f.reply(); require(f.starts == 1 && f.launches == 0);
        }
        try (Fixture f=new Fixture()) {
            // Service destruction/recreation cannot reuse the process owner's stopped admission.
            f.open(); f.drain(); f.nativeStop(); f.browser.close(); f.drain();
            SetupForeground replacement=new SetupForeground(f.now::get);
            require(!replacement.begin("new",f.execution.admission.isOpen(),new SetupForeground.Effects() {
                public boolean start() { throw new AssertionError("revived stopped runtime"); }
                public void finish() { throw new AssertionError("never acquired"); }
            }));
            f.reply(); require(f.launches == 0 && f.finishes == 1);
        }
        System.out.println("services browser lifecycle passed");
    }
}
