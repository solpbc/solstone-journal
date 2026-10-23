// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

public final class OwnerOriginTest {
    static void require(boolean value) { if (!value) throw new AssertionError(); }
    public static void main(String[] args) {
        String base=OwnerOrigin.ORIGIN;
        require(OwnerOrigin.path(base+"/init?state=one#part").equals("/init?state=one"));
        require(OwnerOrigin.redirect(base+"/app/backup/","../../init").equals(base+"/init"));
        for(String value:new String[]{"https://other.invalid/","https://journal.invalid.evil/",
            "https://journal.invalid@other.invalid/","https://journal.invalid:443/",
            "http://journal.invalid/","file:///etc/passwd","content://journal.invalid/",
            "//journal.invalid/","https://journal.invalid//outside/"}) {
            try { OwnerOrigin.admit(value); throw new AssertionError("admitted external or ambiguous origin"); }
            catch(IllegalArgumentException expected) {}
        }
        try { OwnerOrigin.redirect(base+"/init","//other.invalid/"); throw new AssertionError(); }
        catch(IllegalArgumentException expected) {}
        System.out.println("private origin admission and redirect controls passed");
    }
}
