// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package android.app;
public class AlertDialog {
    public static volatile int shown;
    public static java.util.concurrent.CountDownLatch displayed=new java.util.concurrent.CountDownLatch(1);
    public interface Listener { void clicked(Object dialog,int button); }
    public static class Builder {
        public Builder(Object context) {}
        public Builder setTitle(String text) { return this; }
        public Builder setMessage(String text) { return this; }
        public Builder setNegativeButton(String text,Listener listener) { return this; }
        public Builder setPositiveButton(String text,Listener listener) { return this; }
        public void show() { shown++;displayed.countDown(); }
    }
}
