// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

public final class OwnerFolderPickerTest {
    static void require(boolean value) { if (!value) throw new AssertionError(); }

    public static void main(String[] args) {
        staleResultDoesNotStageIntoReplacement();
        resultPageIsCheckedAtCompletion();
        cancelThenReopen();
        uiClosureAfterRetireDoesNotStart();
        presentWhileOpenIsRefused();
        publishFailsWhenGenerationMoves();
        System.out.println("owner folder picker passed");
    }

    static void staleResultDoesNotStageIntoReplacement() {
        OwnerFolderPicker picker = new OwnerFolderPicker();
        require(picker.present("page-a", "page-a", 1) != null);
        require(picker.claimStart("page-a", 1));
        picker.retire();
        require(picker.acceptResult(true, "page-b", 2) == null);
        OwnerFolderPicker.Authority reopened = picker.present("page-b", "page-b", 2);
        require(reopened != null);
        require("page-b".equals(reopened.token));
        require(reopened.generation == 2);
    }

    static void resultPageIsCheckedAtCompletion() {
        OwnerFolderPicker picker = new OwnerFolderPicker();
        require(picker.present("other", "page-a", 1) == null);
        require(picker.present("page-a", "page-a", 1) != null);
        require(picker.claimStart("page-a", 1));
        require(picker.acceptResult(true, "page-b", 2) == null);
        require(picker.present("page-b", "page-b", 2) != null);
    }

    static void cancelThenReopen() {
        OwnerFolderPicker picker = new OwnerFolderPicker();
        require(picker.present("page-a", "page-a", 1) != null);
        require(picker.claimStart("page-a", 1));
        require(picker.acceptResult(false, "page-a", 1) == null);
        require(picker.present("page-a", "page-a", 1) != null);
        require(picker.claimStart("page-a", 1));
        OwnerFolderPicker.Authority origin = picker.acceptResult(true, "page-a", 1);
        require(origin != null);
        require("page-a".equals(origin.token));
        require(origin.generation == 1);
    }

    static void uiClosureAfterRetireDoesNotStart() {
        OwnerFolderPicker picker = new OwnerFolderPicker();
        require(picker.present("page-a", "page-a", 1) != null);
        picker.retire();
        require(!picker.claimStart("page-a", 1));
        require(!picker.claimStart("page-b", 2));
        require(picker.present("page-b", "page-b", 2) != null);
        require(picker.claimStart("page-b", 2));
    }

    static void presentWhileOpenIsRefused() {
        OwnerFolderPicker picker = new OwnerFolderPicker();
        require(picker.present("page-a", "page-a", 1) != null);
        require(picker.present("page-a", "page-a", 1) == null);
        require(picker.claimStart("page-a", 1));
        require(picker.present("page-a", "page-a", 1) == null);
        require(picker.present("", "page-a", 1) == null);
    }

    static void publishFailsWhenGenerationMoves() {
        require(OwnerFolderPicker.same("page-a", 1, "page-a", 1));
        require(!OwnerFolderPicker.same("page-a", 1, "page-a", 2));
        require(!OwnerFolderPicker.same("page-a", 1, "page-b", 1));
        require(!OwnerFolderPicker.same(null, 1, null, 1));
    }
}
