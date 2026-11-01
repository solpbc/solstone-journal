//go:build darwin || ios

// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

/*
#include <fcntl.h>
#include <sys/param.h>

static int archive_source_path(int fd, char *path) {
	return fcntl(fd, F_GETPATH, path);
}
*/
import "C"

import (
	"os"
	"runtime"
)

func adoptArchiveSourceRoot(pinned *os.File, entry *ownedCall, stage *archiveStartStage) (*os.Root, error) {
	if entry != nil && entry.failed() {
		stage.at(archiveStartSourceRootCustody)
		return nil, errOwnedCleanup
	}
	// iOS does not promise a /dev/fd namespace. Resolve the held descriptor
	// through the SDK, then bind the opened root to that same live identity.
	// A rename or replacement after this lookup must still pass SameFile.
	var path [C.MAXPATHLEN]C.char
	result, _ := C.archive_source_path(C.int(pinned.Fd()), &path[0])
	runtime.KeepAlive(pinned)
	if result < 0 || path[0] != '/' {
		return nil, errPinnedIdentity
	}
	for length, character := range path {
		if character == 0 {
			return adoptOwnedRootDiagnosed(C.GoStringN(&path[0], C.int(length)), pinned, entry, stage)
		}
	}
	return nil, errPinnedIdentity
}
