// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

public final class OwnerBytesTest {
    static void require(boolean value) { if (!value) throw new AssertionError(); }
    public static void main(String[] args) {
        byte[] bytes = OwnerBytes.latin1("\u0000\u00ffA");
        require(bytes.length == 3);
        require(bytes[0] == 0);
        require((bytes[1] & 0xff) == 255);
        require(bytes[2] == 'A');
        try { OwnerBytes.latin1("\u0100"); throw new AssertionError("non-byte accepted"); }
        catch (IllegalArgumentException expected) {}
        char[] big = new char[OwnerBytes.MAX_CHUNK + 1];
        try { OwnerBytes.latin1(new String(big)); throw new AssertionError("oversize accepted"); }
        catch (IllegalArgumentException expected) {}
        System.out.println("owner bytes passed");
    }
}
