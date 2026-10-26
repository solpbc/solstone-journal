// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

public final class OwnerBridgeFilesTest {
    static void require(boolean value) { if (!value) throw new AssertionError(); }
    public static void main(String[] args) throws Exception {
        String bridge = "/cache/owner-bridge/8";
        String admitted = "/cache/owner-bridge/8";
        String imports = "/journal/imports/1700000000";
        require(!OwnerBridgeFiles.abandoned("notes", "/cache/owner-bridge/notes", false, false, new String[]{}, new long[]{}));
        require(!OwnerBridgeFiles.abandoned("notes", "/cache/owner-bridge/notes", false, true, new String[]{}, new long[]{}));
        require(!OwnerBridgeFiles.abandoned("9", "/cache/owner-bridge/9", false, false, new String[]{}, new long[]{}));
        require(OwnerBridgeFiles.abandoned("9", "/cache/owner-bridge/9", false, true, new String[]{}, new long[]{}));
        require(!OwnerBridgeFiles.abandoned("9", "/cache/owner-bridge/9", false, true, new String[]{"/cache/owner-bridge/9"}, new long[]{}));
        require(!OwnerBridgeFiles.abandoned("7", "/cache/owner-bridge/7", false, true, new String[]{}, new long[]{7}));
        require(!OwnerBridgeFiles.abandoned("8", bridge, true, false, new String[]{admitted}, new long[]{}));
        require(!OwnerBridgeFiles.abandoned("7", "/cache/owner-bridge/7", true, false, new String[]{}, new long[]{7}));
        require(OwnerBridgeFiles.abandoned("9", "/cache/owner-bridge/9", true, false, new String[]{admitted, imports + "/import.json"}, new long[]{7}));
        require(!OwnerBridgeFiles.abandoned("80", "/cache/owner-bridge/80", true, false, new String[]{"/cache/owner-bridge/80/note.md"}, new long[]{}));
        require(OwnerBridgeFiles.abandoned("8", "/cache/owner-bridge/8", true, false, new String[]{"/cache/owner-bridge/80/note.md"}, new long[]{}));
        restartedCounterFailureLeavesAdmittedCopy();
        System.out.println("owner bridge files passed");
    }

    static void restartedCounterFailureLeavesAdmittedCopy() throws Exception {
        java.io.File tmp = java.nio.file.Files.createTempDirectory("owner-bridge").toFile();
        try {
            java.io.File cache = new java.io.File(tmp, "cache");
            java.io.File journal = new java.io.File(tmp, "journal");
            java.io.File admitted = OwnerBridgeFiles.directory(cache, 1);
            java.io.File note = new java.io.File(new java.io.File(admitted, "notes"), "a.md");
            note.getParentFile().mkdirs();
            byte[] body = new byte[] {'k', 0, (byte) 0xff};
            java.nio.file.Files.write(note.toPath(), body);
            java.io.File receipt = new java.io.File(admitted.getParentFile(), "1.receipt");
            byte[] receiptBody = "{\"id\":1}\n".getBytes(java.nio.charset.StandardCharsets.UTF_8);
            java.nio.file.Files.write(receipt.toPath(), receiptBody);
            java.io.File record = new java.io.File(new java.io.File(journal, "imports"), "20260101_010101");
            record.mkdirs();
            String path = admitted.getAbsolutePath().replace("\\", "\\\\").replace("\"", "\\\"");
            byte[] metadata = ("{\"file_path\":\"" + path + "\"}").getBytes(java.nio.charset.StandardCharsets.UTF_8);
            java.nio.file.Files.write(new java.io.File(record, "import.json").toPath(), metadata);

            OwnerBridgeFiles.Claim claim = OwnerBridgeFiles.allocate(cache);
            require(claim.id != 1);
            java.nio.file.Files.write(new java.io.File(OwnerBridgeFiles.directory(cache, claim.id), "new.md").toPath(), new byte[] {'t'});
            OwnerBridgeFiles.release(cache, claim);

            require(java.util.Arrays.equals(java.nio.file.Files.readAllBytes(note.toPath()), body));
            require(java.util.Arrays.equals(java.nio.file.Files.readAllBytes(receipt.toPath()), receiptBody));
            require(java.util.Arrays.equals(java.nio.file.Files.readAllBytes(new java.io.File(record, "import.json").toPath()), metadata));
            require(!OwnerBridgeFiles.directory(cache, claim.id).exists());
            require(!new java.io.File(admitted.getParentFile(), claim.id + ".claim").exists());

            OwnerBridgeFiles.Claim forged = new OwnerBridgeFiles.Claim(1, "not-the-owner");
            forged.receiptCreated = true;
            OwnerBridgeFiles.release(cache, forged);
            try {
                OwnerBridgeFiles.commit(cache, forged);
                require(false);
            } catch (java.io.IOException expected) { /* An existing receipt is not replaced. */ }
            require(java.util.Arrays.equals(java.nio.file.Files.readAllBytes(note.toPath()), body));
            require(java.util.Arrays.equals(java.nio.file.Files.readAllBytes(receipt.toPath()), receiptBody));
            require(java.util.Arrays.equals(java.nio.file.Files.readAllBytes(new java.io.File(record, "import.json").toPath()), metadata));
        } finally {
            delete(tmp);
        }
    }

    static void delete(java.io.File file) {
        java.io.File[] children = file.listFiles();
        if (children != null) for (java.io.File child : children) delete(child);
        file.delete();
    }
}
