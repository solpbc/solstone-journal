// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

/** Latin-1 bytes from a bridge string. NUL and 0xFF survive; a larger chunk does not. */
final class OwnerBytes {
    static final int MAX_CHUNK = 65536;

    static byte[] latin1(String text) {
        if (text == null) throw new IllegalArgumentException("chunk required");
        if (text.length() > MAX_CHUNK) throw new IllegalArgumentException("chunk exceeds transfer budget");
        byte[] bytes = new byte[text.length()];
        for (int index = 0; index < text.length(); index++) {
            char unit = text.charAt(index);
            if (unit > 255) throw new IllegalArgumentException("chunk is not a byte string");
            bytes[index] = (byte) unit;
        }
        return bytes;
    }
}
