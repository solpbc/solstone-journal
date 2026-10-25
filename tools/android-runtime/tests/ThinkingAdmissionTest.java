// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;
import java.util.concurrent.*;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Map;
import java.util.HashMap;

// Standalone host lifecycle check; no Android or provider stand-in is involved.
public final class ThinkingAdmissionTest {
    static void require(boolean value) { if (!value) throw new AssertionError(); }
    static Map<String, Object> object(Object... pairs) {
        Map<String, Object> value = new HashMap<>();
        for (int i = 0; i < pairs.length; i += 2) value.put((String)pairs[i], pairs[i+1]);
        return value;
    }
    static ServingConnection.Retirement retirement(Object response) {
        return ServingConnection.retirement(response, (row, key) ->
            row instanceof Map ? ((Map<?, ?>)row).get(key) : null);
    }
    public static void main(String[] args) throws Exception {
        require(retirement(object("native", object("ok", true, "value", object("stopping", true, "failed", false)))) == ServingConnection.Retirement.PENDING);
        require(retirement(object("native", object("ok", true, "value", object("stopping", false, "failed", false)))) == ServingConnection.Retirement.STOPPED);
        require(retirement(object("native", object("ok", true, "value", object("stopping", true, "failed", true)))) == ServingConnection.Retirement.FAILED);
        require(retirement(object("native", object("ok", false, "error", "server stopped"))) == ServingConnection.Retirement.STOPPED);
        require(retirement(object("native", object("ok", false, "error", "retirement unreadable"))) == ServingConnection.Retirement.FAILED);
        require(retirement(object("native", object("ok", true))) == ServingConnection.Retirement.FAILED);
        require(retirement(object("native", object("ok", true, "value", 5))) == ServingConnection.Retirement.FAILED);
        require(retirement(object("native", object("ok", true, "value", object("root_current", true)))) == ServingConnection.Retirement.STOPPED);
        for (String phase : new String[]{"starting", "running", "stopping", "stopunknown", "stopfailed", "recovering"})
            require(!ServingConnection.ownerStartAllowed(phase,false,false,true));
        require(ServingConnection.ownerStartAllowed("stopped",false,false,true));
        require(ServingConnection.ownerStartAllowed("failed",false,false,true));
        require(!ServingConnection.ownerStartAllowed("stopped",false,true,true));
        require(!ServingConnection.ownerStartAllowed("stopped",false,false,false));
        require(!ServingConnection.ownerStartAllowed("stopped",true,false,true));

        // Hold the real lifecycle queue: ordinary Stop must signal cancellation
        // before that queue becomes available, without retiring the active owner.
        JournalExecution early = new JournalExecution();
        early.admission.opened(early.admission.generation());
        ThinkingAdmission.Ticket accepted = early.admission.admit();
        CountDownLatch controlHeld = new CountDownLatch(1), releaseControl = new CountDownLatch(1);
        CountDownLatch nativeCancelled = new CountDownLatch(1), retired = new CountDownLatch(1);
        early.work.execute(()->{
            controlHeld.countDown();
            try { releaseControl.await(); } catch(InterruptedException error) { throw new AssertionError(error); }
        });
        require(controlHeld.await(5,TimeUnit.SECONDS));
        try {
            ServingConnection.stop(early.work,()->early.beginStop(()->nativeCancelled.countDown()),()->{
                early.admission.awaitIdle(); retired.countDown();
            });
            require(nativeCancelled.getCount() == 0);
            require(!accepted.current());
            require(early.admission.admit() == null);
            require(retired.getCount() == 1);
            // Repeating Stop cannot permit a replacement before the same owner drains.
            ServingConnection.stop(early.work,()->early.beginStop(()->{}),()->early.admission.awaitIdle());
            releaseControl.countDown();
            require(!retired.await(100,TimeUnit.MILLISECONDS));
        } finally {
            releaseControl.countDown(); accepted.close();
            early.work.shutdown(); early.thinking.shutdown();
        }
        require(retired.await(5,TimeUnit.SECONDS));
        require(early.work.awaitTermination(5,TimeUnit.SECONDS));

        ThinkingAdmission admission = new ThinkingAdmission();
        require(admission.admit() == null);
        long first = admission.generation(); admission.opened(first);
        ThinkingAdmission.Ticket active = admission.admit();
        require(active != null && active.current());
        require(admission.admit() == null); // No second request can enter an executor queue.
        admission.stop(); require(!active.current()); require(admission.admit() == null);
        admission.opened(first); require(admission.admit() == null); // A stale start cannot reopen.
        CountDownLatch waiting = new CountDownLatch(1);
        FutureTask<Void> stop = new FutureTask<>(()->{ waiting.countDown(); admission.awaitIdle(); return null; });
        Thread thread = new Thread(stop); thread.start(); waiting.await();
        try { stop.get(100, TimeUnit.MILLISECONDS); throw new AssertionError("retired active request"); }
        catch (TimeoutException expected) { /* The accepted owner is still held. */ }
        active.close(); active.close(); // Repeated release must not mint another permit.
        stop.get(5, TimeUnit.SECONDS); thread.join();
        admission.opened(admission.generation());
        ThinkingAdmission.Ticket fresh = admission.admit();
        require(fresh != null && fresh.current() && !active.current());
        require(admission.admit() == null);
        fresh.close(); admission.stop(); admission.awaitIdle();

        // Two Java service lifetimes share the actual process execution owner.
        // Hold its queue to reproduce an old explicit stop and destruction
        // arriving before a replacement service submits startup.
        JournalExecution execution = new JournalExecution();
        CountDownLatch queueHeld = new CountDownLatch(1), releaseQueue = new CountDownLatch(1);
        AtomicInteger running = new AtomicInteger(1);
        ArrayList<String> order = new ArrayList<>();
        execution.work.execute(()->{
            queueHeld.countDown();
            try { releaseQueue.await(); }
            catch (InterruptedException e) { throw new AssertionError(e); }
        });
        queueHeld.await();
        execution.admission.opened(execution.admission.generation());
        ThinkingAdmission.Ticket old = execution.admission.admit();
        execution.admission.stop();
        execution.work.execute(()->{
            execution.admission.awaitIdle(); running.set(0); order.add("explicit stop");
        });
        execution.retire(() -> {}, ()->{ running.set(0); order.add("old destruction"); });
        long replacementGeneration = execution.admission.generation();
        Future<?> replacement = execution.work.submit(()->{
            running.set(2); order.add("replacement start");
            execution.admission.opened(replacementGeneration);
        });
        releaseQueue.countDown();
        try { replacement.get(100, TimeUnit.MILLISECONDS); throw new AssertionError("replaced active request"); }
        catch (TimeoutException expected) { /* The old owner still holds its request. */ }
        old.close(); replacement.get(5, TimeUnit.SECONDS);
        execution.work.submit(()->{}).get(5, TimeUnit.SECONDS);
        require(running.get() == 2);
        require(order.equals(Arrays.asList("explicit stop", "old destruction", "replacement start")));
        ThinkingAdmission.Ticket replacementTicket = execution.admission.admit();
        require(replacementTicket != null && replacementTicket.current());
        replacementTicket.close();
        execution.work.shutdown(); execution.thinking.shutdown();

        // Latch test: hold ticket, call retire(cancel, stop).
        // cancel runs while awaitIdle is blocked, releasing ticket lets stop run.
        JournalExecution latchExec = new JournalExecution();
        latchExec.admission.opened(latchExec.admission.generation());
        ThinkingAdmission.Ticket heldTicket = latchExec.admission.admit();
        require(heldTicket != null);
        CountDownLatch cancelRan = new CountDownLatch(1);
        CountDownLatch stopRan = new CountDownLatch(1);
        latchExec.retire(() -> cancelRan.countDown(), () -> stopRan.countDown());
        require(cancelRan.await(1, TimeUnit.SECONDS));
        require(stopRan.getCount() == 1);
        heldTicket.close();
        require(stopRan.await(5, TimeUnit.SECONDS));
        latchExec.work.shutdown(); latchExec.thinking.shutdown();

        // ServingConnection lifecycle scenarios with controllable callbacks
        // Refused start: native start returns false -> REFUSE, no audio, no ready
        {
            AtomicInteger starts = new AtomicInteger();
            AtomicInteger audios = new AtomicInteger();
            AtomicInteger readies = new AtomicInteger();
            ServingConnection.Action action = ServingConnection.drive(1L, new ServingConnection.Environment() {
                public long currentGeneration() { return 1L; }
                public ServingConnection.Retirement retirement() { return ServingConnection.Retirement.STOPPED; }
                public boolean start() { starts.incrementAndGet(); return false; }
                public boolean enableAudio() { audios.incrementAndGet(); return true; }
                public void onReady(long gen) { readies.incrementAndGet(); }
            });
            require(action == ServingConnection.Action.REFUSE);
            require(starts.get() == 1);
            require(audios.get() == 0);
            require(readies.get() == 0);
        }

        // Pending then stopped: pending returns RETRY without start; then stopped starts once and readies
        {
            AtomicInteger starts = new AtomicInteger();
            AtomicInteger audios = new AtomicInteger();
            AtomicInteger readies = new AtomicInteger();
            ServingConnection.Environment envPending = new ServingConnection.Environment() {
                public long currentGeneration() { return 1L; }
                public ServingConnection.Retirement retirement() { return ServingConnection.Retirement.PENDING; }
                public boolean start() { starts.incrementAndGet(); return true; }
                public boolean enableAudio() { audios.incrementAndGet(); return true; }
                public void onReady(long gen) { readies.incrementAndGet(); }
            };
            ServingConnection.Action retryAction = ServingConnection.drive(1L, envPending);
            require(retryAction == ServingConnection.Action.RETRY);
            require(starts.get() == 0);
            require(audios.get() == 0);
            require(readies.get() == 0);

            ServingConnection.Environment envStopped = new ServingConnection.Environment() {
                public long currentGeneration() { return 1L; }
                public ServingConnection.Retirement retirement() { return ServingConnection.Retirement.STOPPED; }
                public boolean start() { starts.incrementAndGet(); return true; }
                public boolean enableAudio() { audios.incrementAndGet(); return true; }
                public void onReady(long gen) { readies.incrementAndGet(); }
            };
            ServingConnection.Action readyAction = ServingConnection.drive(1L, envStopped);
            require(readyAction == ServingConnection.Action.READY);
            require(starts.get() == 1);
            require(audios.get() == 1);
            require(readies.get() == 1);
        }

        // Stale generation cannot ready or stop: DROP
        {
            AtomicInteger starts = new AtomicInteger();
            AtomicInteger readies = new AtomicInteger();
            ServingConnection.Action action = ServingConnection.drive(1L, new ServingConnection.Environment() {
                public long currentGeneration() { return 2L; }
                public ServingConnection.Retirement retirement() { return ServingConnection.Retirement.STOPPED; }
                public boolean start() { starts.incrementAndGet(); return true; }
                public boolean enableAudio() { return true; }
                public void onReady(long gen) { readies.incrementAndGet(); }
            });
            require(action == ServingConnection.Action.DROP);
            require(starts.get() == 0);
            require(readies.get() == 0);

            ServingConnection.Action stopAction = ServingConnection.decide(1L, 2L, true, true, true, ServingConnection.Retirement.STOPPED);
            require(stopAction == ServingConnection.Action.DROP);
        }

        // Both successes ready
        {
            AtomicInteger starts = new AtomicInteger();
            AtomicInteger audios = new AtomicInteger();
            AtomicInteger readies = new AtomicInteger();
            ServingConnection.Action action = ServingConnection.drive(3L, new ServingConnection.Environment() {
                public long currentGeneration() { return 3L; }
                public ServingConnection.Retirement retirement() { return ServingConnection.Retirement.STOPPED; }
                public boolean start() { starts.incrementAndGet(); return true; }
                public boolean enableAudio() { audios.incrementAndGet(); return true; }
                public void onReady(long gen) { readies.incrementAndGet(); }
            });
            require(action == ServingConnection.Action.READY);
            require(starts.get() == 1);
            require(audios.get() == 1);
            require(readies.get() == 1);
        }

        // Audio failure not ready
        {
            AtomicInteger starts = new AtomicInteger();
            AtomicInteger audios = new AtomicInteger();
            AtomicInteger readies = new AtomicInteger();
            ServingConnection.Action action = ServingConnection.drive(4L, new ServingConnection.Environment() {
                public long currentGeneration() { return 4L; }
                public ServingConnection.Retirement retirement() { return ServingConnection.Retirement.STOPPED; }
                public boolean start() { starts.incrementAndGet(); return true; }
                public boolean enableAudio() { audios.incrementAndGet(); return false; }
                public void onReady(long gen) { readies.incrementAndGet(); }
            });
            require(action != ServingConnection.Action.READY);
            require(starts.get() == 1);
            require(audios.get() == 1);
            require(readies.get() == 0);
        }

        System.out.println("thinking admission lifecycle passed");
    }
}
