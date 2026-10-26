// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import java.io.Closeable;
import java.io.IOException;

public final class OwnerPipePairTest {
    static final class End implements Closeable {
        int closed;
        public void close() { closed++; }
    }

    static void require(boolean value) { if (!value) throw new AssertionError(); }

    public static void main(String[] args) throws Exception {
        secondOpenFailureClosesFirstPair();
        shortSecondPairClosesFirstPair();
        runtimeFailureWrapsAndClosesFirstPair();
        successLeavesBothPairsOpen();
        System.out.println("owner pipe pair passed");
    }

    static void secondOpenFailureClosesFirstPair() {
        final End[] first = new End[2];
        final int[] calls = new int[1];
        try {
            OwnerPipePair.openTwo(new OwnerPipePair.Opener<End>() {
                public End[] open() throws IOException {
                    calls[0]++;
                    if (calls[0] == 1) {
                        first[0] = new End();
                        first[1] = new End();
                        return new End[] {first[0], first[1]};
                    }
                    throw new IOException("second pipe failed");
                }
            });
            throw new AssertionError("second open must fail");
        } catch (IOException error) {
            require("second pipe failed".equals(error.getMessage()));
        }
        require(calls[0] == 2);
        require(first[0].closed == 1);
        require(first[1].closed == 1);
    }

    static void shortSecondPairClosesFirstPair() {
        final End[] first = new End[2];
        final End stray = new End();
        try {
            OwnerPipePair.openTwo(new OwnerPipePair.Opener<End>() {
                int calls;
                public End[] open() {
                    calls++;
                    if (calls == 1) {
                        first[0] = new End();
                        first[1] = new End();
                        return new End[] {first[0], first[1]};
                    }
                    return new End[] {stray};
                }
            });
            throw new AssertionError("short second pair must fail");
        } catch (IOException error) {
            require("journal response unavailable; check the result before trying again".equals(error.getMessage()));
        }
        require(first[0].closed == 1);
        require(first[1].closed == 1);
        require(stray.closed == 1);
    }

    static void runtimeFailureWrapsAndClosesFirstPair() {
        final End[] first = new End[2];
        try {
            OwnerPipePair.openTwo(new OwnerPipePair.Opener<End>() {
                int calls;
                public End[] open() {
                    calls++;
                    if (calls == 1) {
                        first[0] = new End();
                        first[1] = new End();
                        return new End[] {first[0], first[1]};
                    }
                    throw new IllegalStateException("boom");
                }
            });
            throw new AssertionError("runtime failure must fail closed");
        } catch (IOException error) {
            require("journal response unavailable; check the result before trying again".equals(error.getMessage()));
        }
        require(first[0].closed == 1);
        require(first[1].closed == 1);
    }

    static void successLeavesBothPairsOpen() throws IOException {
        final End[] opened = new End[4];
        final int[] calls = new int[1];
        OwnerPipePair.Ends<End> ends = OwnerPipePair.openTwo(new OwnerPipePair.Opener<End>() {
            public End[] open() {
                int index = calls[0] * 2;
                calls[0]++;
                opened[index] = new End();
                opened[index + 1] = new End();
                return new End[] {opened[index], opened[index + 1]};
            }
        });
        require(calls[0] == 2);
        require(ends.bodyRead == opened[0] && opened[0].closed == 0);
        require(ends.bodyWrite == opened[1] && opened[1].closed == 0);
        require(ends.responseRead == opened[2] && opened[2].closed == 0);
        require(ends.responseWrite == opened[3] && opened[3].closed == 0);
    }
}
