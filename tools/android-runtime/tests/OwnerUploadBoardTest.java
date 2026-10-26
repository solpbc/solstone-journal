// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import java.util.concurrent.CyclicBarrier;
import java.util.concurrent.atomic.AtomicInteger;

public final class OwnerUploadBoardTest {
    static void require(boolean value) { if (!value) throw new AssertionError(); }

    public static void main(String[] args) throws Exception {
        completedRepliesStayUntilTake();
        pendingTakeKeepsTheSlot();
        secondKeepDoesNotReplace();
        cancelAndRetireFreeTheSlot();
        concurrentAdmitsStopAtTheBound();
        System.out.println("owner upload board passed");
    }

    static void completedRepliesStayUntilTake() {
        OwnerUploadBoard board = new OwnerUploadBoard();
        for (int index = 0; index < OwnerUploadBoard.BOUND; index++) {
            String key = "done-" + index;
            require(board.admit(key));
            require(board.keep(key, "reply-" + index));
        }
        require(board.held() == OwnerUploadBoard.BOUND);
        require(!board.admit("overflow"));
        require(board.held() == OwnerUploadBoard.BOUND);
        OwnerUploadBoard.Taken first = board.take("done-0");
        require(!first.pending);
        require("reply-0".equals(first.reply));
        require(board.held() == OwnerUploadBoard.BOUND - 1);
        require(board.admit("overflow"));
        require(board.held() == OwnerUploadBoard.BOUND);
    }

    static void pendingTakeKeepsTheSlot() {
        OwnerUploadBoard board = new OwnerUploadBoard();
        require(board.admit("open"));
        OwnerUploadBoard.Taken pending = board.take("open");
        require(pending.pending);
        require(pending.reply == null);
        require(board.held() == 1);
        require(!board.admit("open"));
        require(board.keep("open", "later"));
        OwnerUploadBoard.Taken ready = board.take("open");
        require(!ready.pending);
        require("later".equals(ready.reply));
        require(board.held() == 0);
        OwnerUploadBoard.Taken absent = board.take("open");
        require(!absent.pending);
        require(absent.reply == null);
    }

    static void secondKeepDoesNotReplace() {
        OwnerUploadBoard board = new OwnerUploadBoard();
        require(board.admit("same"));
        require(board.keep("same", "first"));
        require(board.keep("same", "second"));
        require(!board.keep("same", null));
        require(!board.keep("missing", "nope"));
        require("first".equals(board.take("same").reply));
    }

    static void cancelAndRetireFreeTheSlot() {
        OwnerUploadBoard board = new OwnerUploadBoard();
        require(board.admit("cancel-me"));
        require(board.keep("cancel-me", "held"));
        board.cancel("cancel-me");
        require(board.held() == 0);
        require(board.take("cancel-me").reply == null);
        require(board.admit("again"));
        board.retire();
        require(board.held() == 0);
        require(board.admit("after-retire"));
        require(board.held() == 1);
    }

    static void concurrentAdmitsStopAtTheBound() throws Exception {
        final OwnerUploadBoard board = new OwnerUploadBoard();
        final int racers = OwnerUploadBoard.BOUND * 2;
        final CyclicBarrier start = new CyclicBarrier(racers);
        final AtomicInteger wins = new AtomicInteger();
        Thread[] threads = new Thread[racers];
        for (int index = 0; index < racers; index++) {
            final String key = "race-" + index;
            threads[index] = new Thread(new Runnable() {
                public void run() {
                    try { start.await(); }
                    catch (Exception error) { throw new RuntimeException(error); }
                    if (board.admit(key)) wins.incrementAndGet();
                }
            });
            threads[index].start();
        }
        for (Thread thread : threads) thread.join();
        require(wins.get() == OwnerUploadBoard.BOUND);
        require(board.held() == OwnerUploadBoard.BOUND);
        board.retire();
        require(board.held() == 0);
    }
}
