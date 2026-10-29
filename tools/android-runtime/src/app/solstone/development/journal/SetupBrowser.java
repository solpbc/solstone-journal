// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import java.net.URI;
import java.util.UUID;

/** Main-thread owner of one user-initiated services-browser departure. */
final class SetupBrowser {
    interface Connection {
        boolean current();
        void acquire(String lease) throws Exception;
        void release(String lease);
        void launch(String url) throws Exception;
        void refused();
    }
    private static final class Open {
        final String id = UUID.randomUUID().toString();
        final Connection connection;
        final String url;
        boolean launched;
        Open(Connection connection, String url) { this.connection=connection; this.url=url; }
    }
    private boolean resumed;
    private Open open;

    static boolean protectedUrl(String value) {
        try {
            URI uri=URI.create(value);
            return "https".equals(uri.getScheme()) && "services.solstone.app".equals(uri.getHost())
                && uri.getRawUserInfo() == null && (uri.getPort() == -1 || uri.getPort() == 443);
        } catch (IllegalArgumentException error) { return false; }
    }
    void resume() { resumed=true; close(); }
    void pause() {
        resumed=false;
        // A successful dispatch causes its own pause. A pending acquisition
        // must never open a browser later, over Home or another activity.
        if (open != null && !open.launched) close();
    }
    void close() {
        Open retired=open; open=null;
        if (retired != null) retired.connection.release(retired.id);
    }
    void begin(Connection connection, String url) {
        close();
        if (!resumed || !connection.current() || !protectedUrl(url)) { connection.refused(); return; }
        Open next=new Open(connection,url); open=next;
        try { connection.acquire(next.id); }
        catch (Exception error) { close(); connection.refused(); }
    }
    void acknowledged(String id, boolean acquired) {
        Open next=open;
        if (next == null || !next.id.equals(id) || next.launched) return;
        if (!acquired || !resumed || !next.connection.current()) {
            close(); next.connection.refused(); return;
        }
        try {
            // Set before dispatch: the launch's onPause must retain this lease.
            next.launched=true;
            next.connection.launch(next.url);
        } catch (Exception error) { close(); next.connection.refused(); }
    }
    void acquisitionExpired(String id) {
        if (open != null && open.id.equals(id) && !open.launched) {
            Connection connection=open.connection;
            close(); connection.refused();
        }
    }
}
