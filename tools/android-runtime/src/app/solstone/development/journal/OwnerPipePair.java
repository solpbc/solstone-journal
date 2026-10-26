// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import java.io.Closeable;
import java.io.IOException;

/** Two pipe pairs. The first pair is closed when the second open fails. */
final class OwnerPipePair {
    interface Opener<T extends Closeable> {
        T[] open() throws IOException;
    }

    static final class Ends<T extends Closeable> {
        final T bodyRead;
        final T bodyWrite;
        final T responseRead;
        final T responseWrite;

        Ends(T bodyRead, T bodyWrite, T responseRead, T responseWrite) {
            this.bodyRead = bodyRead;
            this.bodyWrite = bodyWrite;
            this.responseRead = responseRead;
            this.responseWrite = responseWrite;
        }
    }

    static <T extends Closeable> Ends<T> openTwo(Opener<T> opener) throws IOException {
        T[] body = opener.open();
        if (!pair(body)) {
            close(body);
            throw new IOException("journal response unavailable; check the result before trying again");
        }
        try {
            T[] response = opener.open();
            if (!pair(response)) {
                close(response);
                throw new IOException("journal response unavailable; check the result before trying again");
            }
            return new Ends<T>(body[0], body[1], response[0], response[1]);
        } catch (Exception error) {
            close(body);
            if (error instanceof IOException) throw (IOException) error;
            throw new IOException("journal response unavailable; check the result before trying again");
        }
    }

    private static boolean pair(Closeable[] pipes) {
        return pipes != null && pipes.length == 2 && pipes[0] != null && pipes[1] != null;
    }

    private static void close(Closeable[] pipes) {
        if (pipes == null) return;
        for (int index = 0; index < pipes.length; index++) close(pipes[index]);
    }

    private static void close(Closeable pipe) {
        if (pipe == null) return;
        try { pipe.close(); } catch (IOException ignored) { /* The failed open still reports the second failure. */ }
    }
}
