// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

public final class OwnerAcceptTest {
    static void require(boolean value) { if (!value) throw new AssertionError(); }
    public static void main(String[] args) {
        require("*/*".equals(OwnerAccept.mime("")));
        require("*/*".equals(OwnerAccept.mime(null)));
        require("application/zip".equals(OwnerAccept.mime(".zip")));
        require("application/pdf".equals(OwnerAccept.mime(".pdf")));
        require("image/*".equals(OwnerAccept.mime(".png,.jpg,.jpeg,.webp,.gif")));
        require("audio/*".equals(OwnerAccept.mime(".flac,.m4a,.mp3,.ogg,.opus,.wav")));
        require("*/*".equals(OwnerAccept.mime(".zip,.json")));
        require("*/*".equals(OwnerAccept.mime(".ics,.zip")));
        require("*/*".equals(OwnerAccept.mime(".zip,.csv")));
        require("*/*".equals(OwnerAccept.mime(".flac,.gif,.jpeg,.jpg,.m4a,.mov,.mp3,.mp4,.ogg,.opus,.png,.wav,.webm,.webp")));
        require("*/*".equals(OwnerAccept.mime(".unknown")));
        System.out.println("owner accept lists passed");
    }
}
