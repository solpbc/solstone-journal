// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import java.util.function.BiFunction;

public final class ServingConnection {
    public enum Action {
        DROP,
        RETRY,
        REFUSE,
        ENABLE_AUDIO,
        READY
    }

    public enum Retirement {
        PENDING,
        STOPPED,
        FAILED
    }

    // Cancellation must reach the worker even while a lifecycle reply holds
    // the control queue. Settlement still uses that queue's existing FIFO.
    public static void stop(java.util.concurrent.Executor control, Runnable cancel, Runnable retire) {
        cancel.run();
        control.execute(retire);
    }

    public static boolean ownerStartAllowed(String phase, boolean preparing, boolean runtimeUp, boolean stopSaved) {
        return !preparing && !runtimeUp && stopSaved
            && ("stopped".equals(phase) || "failed".equals(phase));
    }

    public interface Environment {
        long currentGeneration();
        Retirement retirement() throws Exception;
        boolean start() throws Exception;
        boolean enableAudio() throws Exception;
        void onReady(long generation);
    }

    // The accessor keeps this boundary decoder executable in host lifecycle
    // tests while the activity supplies the real JSONObject accessor.
    public static Retirement retirement(Object response, BiFunction<Object, String, Object> get) {
        Object nativeResult = get.apply(response, "native");
        if (!Boolean.TRUE.equals(get.apply(nativeResult, "ok"))) {
            return "server stopped".equals(get.apply(nativeResult, "error"))
                ? Retirement.STOPPED : Retirement.FAILED;
        }
        Object value = get.apply(nativeResult, "value");
        if (value == null) return Retirement.FAILED;
        Object failed = get.apply(value, "failed"), stopping = get.apply(value, "stopping");
        if ((failed != null && !(failed instanceof Boolean))
            || (stopping != null && !(stopping instanceof Boolean))) return Retirement.FAILED;
        if (failed == null && stopping == null
            && !(get.apply(value, "root_current") instanceof Boolean)) return Retirement.FAILED;
        if (Boolean.TRUE.equals(failed)) return Retirement.FAILED;
        if (Boolean.TRUE.equals(stopping)) return Retirement.PENDING;
        return Retirement.STOPPED;
    }

    public static Action decide(
        long commandGeneration,
        long currentGeneration,
        boolean startAlreadySucceeded,
        Boolean startOk,
        Boolean audioOk,
        Retirement retirement
    ) {
        if (commandGeneration != currentGeneration) {
            return Action.DROP;
        }
        if (retirement == Retirement.FAILED || Boolean.FALSE.equals(startOk)) {
            return Action.REFUSE;
        }
        if (Boolean.FALSE.equals(audioOk)) {
            return Action.REFUSE;
        }
        if (retirement == Retirement.PENDING && !startAlreadySucceeded) {
            return Action.RETRY;
        }
        if (retirement == Retirement.PENDING && startAlreadySucceeded) {
            return Action.RETRY;
        }
        if (startAlreadySucceeded || Boolean.TRUE.equals(startOk)) {
            if (audioOk == null) {
                return Action.ENABLE_AUDIO;
            }
            if (Boolean.TRUE.equals(audioOk)) {
                return Action.READY;
            }
        }
        if (retirement == Retirement.STOPPED && !startAlreadySucceeded) {
            return Action.RETRY;
        }
        return Action.REFUSE;
    }

    public static Action drive(long generation, boolean startAlreadySucceeded, Environment env) throws Exception {
        if (generation != env.currentGeneration()) {
            return Action.DROP;
        }
        Retirement ret = env.retirement();
        if (ret == Retirement.FAILED) {
            return Action.REFUSE;
        }
        if (ret == Retirement.PENDING) {
            return Action.RETRY;
        }
        if (generation != env.currentGeneration()) {
            return Action.DROP;
        }
        if (!startAlreadySucceeded) {
            boolean startOk = env.start();
            if (!startOk) {
                return Action.REFUSE;
            }
            startAlreadySucceeded = true;
        }
        if (generation != env.currentGeneration()) {
            return Action.DROP;
        }
        boolean audioOk = env.enableAudio();
        if (!audioOk) {
            return Action.REFUSE;
        }
        if (generation != env.currentGeneration()) {
            return Action.DROP;
        }
        env.onReady(generation);
        return Action.READY;
    }

    public static Action drive(long generation, Environment env) throws Exception {
        return drive(generation, false, env);
    }
}
