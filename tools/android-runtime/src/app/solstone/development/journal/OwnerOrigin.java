// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import java.net.URI;

/** Virtual origin served exclusively by the app's private owner transport. */
final class OwnerOrigin {
    static final String ORIGIN = "https://journal.invalid";
    static URI admit(String value) {
        URI uri = URI.create(value);
        if (!"https".equals(uri.getScheme()) || !"journal.invalid".equals(uri.getRawAuthority())
                || uri.getRawUserInfo() != null || uri.getPort() != -1
                || uri.getRawPath() == null || !uri.getRawPath().startsWith("/")
                || uri.getRawPath().startsWith("//") || value.length() > 8192) {
            throw new IllegalArgumentException("private journal address required");
        }
        return uri;
    }
    static String path(String value) {
        URI uri = admit(value);
        return uri.getRawPath() + (uri.getRawQuery() == null ? "" : "?"+uri.getRawQuery());
    }
    static String redirect(String source, String target) {
        String value=admit(source).resolve(target).toString();
        admit(value);
        return value;
    }
}
