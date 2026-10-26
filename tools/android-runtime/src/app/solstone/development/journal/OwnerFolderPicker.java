// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

/** The page that opened the folder picker. A later page cannot take its result. */
final class OwnerFolderPicker {
    static final class Authority {
        final String token;
        final int generation;

        Authority(String token, int generation) {
            this.token = token;
            this.generation = generation;
        }
    }

    private Authority origin;
    private boolean armed;
    private boolean open;
    private boolean discardResult;

    synchronized Authority present(String requested, String pageToken, int generation) {
        if (open || armed) return null;
        if (requested == null || pageToken == null || pageToken.isEmpty() || !requested.equals(pageToken)) return null;
        origin = new Authority(pageToken, generation);
        armed = true;
        discardResult = false;
        return origin;
    }

    synchronized boolean live(String pageToken, int generation) {
        return origin != null && !discardResult && pageToken != null
            && pageToken.equals(origin.token) && generation == origin.generation;
    }

    /** The posted UI runnable may start the picker only while the originating page is still showing. */
    synchronized boolean claimStart(String pageToken, int generation) {
        if (!armed || !live(pageToken, generation)) {
            if (!open) {
                armed = false;
                origin = null;
            }
            return false;
        }
        armed = false;
        open = true;
        return true;
    }

    synchronized void retire() {
        origin = null;
        if (open) discardResult = true;
        else armed = false;
    }

    synchronized Authority acceptResult(boolean ok, String pageToken, int generation) {
        Authority captured = origin;
        boolean discard = discardResult || !ok;
        origin = null;
        armed = false;
        open = false;
        discardResult = false;
        if (discard || captured == null) return null;
        if (!same(captured.token, captured.generation, pageToken, generation)) return null;
        return captured;
    }

    static boolean same(String token, int generation, String pageToken, int pageGeneration) {
        return token != null && token.equals(pageToken) && generation == pageGeneration;
    }
}
