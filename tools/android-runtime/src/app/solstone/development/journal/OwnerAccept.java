// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

/** Maps an extension accept list to one MIME type, or an unrestricted picker. */
final class OwnerAccept {
    static String mime(String accept) {
        if (accept == null) return "*/*";
        String[] parts = accept.split(",");
        String specific = null;
        boolean image = false, audio = false, video = false;
        int count = 0;
        for (int index = 0; index < parts.length; index++) {
            String ext = parts[index].trim().toLowerCase();
            if (ext.startsWith(".")) ext = ext.substring(1);
            if (ext.isEmpty()) continue;
            String type = type(ext);
            if (type == null) return "*/*";
            count++;
            if ("image/*".equals(type)) image = true;
            else if ("audio/*".equals(type)) audio = true;
            else if ("video/*".equals(type)) video = true;
            else if (specific == null) specific = type;
            else if (!specific.equals(type)) return "*/*";
        }
        if (count == 0) return "*/*";
        int kinds = (image ? 1 : 0) + (audio ? 1 : 0) + (video ? 1 : 0) + (specific == null ? 0 : 1);
        if (kinds != 1) return "*/*";
        if (image) return "image/*";
        if (audio) return "audio/*";
        if (video) return "video/*";
        return specific;
    }

    private static String type(String ext) {
        if ("zip".equals(ext)) return "application/zip";
        if ("ics".equals(ext)) return "text/calendar";
        if ("json".equals(ext)) return "application/json";
        if ("csv".equals(ext)) return "text/csv";
        if ("pdf".equals(ext)) return "application/pdf";
        if ("png".equals(ext) || "jpg".equals(ext) || "jpeg".equals(ext) || "webp".equals(ext) || "gif".equals(ext)) return "image/*";
        if ("flac".equals(ext) || "m4a".equals(ext) || "mp3".equals(ext) || "ogg".equals(ext) || "opus".equals(ext) || "wav".equals(ext)) return "audio/*";
        if ("mov".equals(ext) || "mp4".equals(ext) || "webm".equals(ext)) return "video/*";
        return null;
    }
}
