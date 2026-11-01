//go:build linux

// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

import (
	"fmt"
	"os"
)

func adoptArchiveSourceRoot(pinned *os.File, entry *ownedCall, stage *archiveStartStage) (*os.Root, error) {
	return adoptOwnedRootDiagnosed(fmt.Sprintf("/dev/fd/%d", int(pinned.Fd())), pinned, entry, stage)
}
