// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

import (
	"os"

	"golang.org/x/sys/unix"
)

// Identity only. The caller keeps its descriptor stable during synchronous
// entry; native work never retains, duplicates, adopts or closes that number.
type borrowedDirectoryIdentity struct {
	device uint64
	inode  uint64
}

func readBorrowedDirectoryIdentity(fd int) (*borrowedDirectoryIdentity, int32) {
	var stat unix.Stat_t
	if fd < 0 || unix.Fstat(fd, &stat) != nil || stat.Mode&unix.S_IFMT != unix.S_IFDIR || stat.Uid != uint32(os.Geteuid()) {
		return nil, StatusInvalidArgument
	}
	return &borrowedDirectoryIdentity{device: uint64(stat.Dev), inode: uint64(stat.Ino)}, StatusOk
}

// Private observation of the actual acquired descriptor before comparison.
// Nil in production; no exported callback or installation authority.
var testDirectoryIdentity func(string, *os.File, *borrowedDirectoryIdentity)

func matchesBorrowedDirectory(kind string, file *os.File, expected *borrowedDirectoryIdentity) bool {
	if expected == nil {
		return true // Only original public entries omit the new expectation.
	}
	if testDirectoryIdentity != nil {
		testDirectoryIdentity(kind, file, expected)
	}
	var stat unix.Stat_t
	return file != nil && unix.Fstat(int(file.Fd()), &stat) == nil && stat.Mode&unix.S_IFMT == unix.S_IFDIR &&
		stat.Uid == uint32(os.Geteuid()) && uint64(stat.Dev) == expected.device && uint64(stat.Ino) == expected.inode
}

func normalDirectoryOperation(operation int32) bool {
	switch operation {
	case OpInit, OpOpen, OpAddRecoveryKey, OpSnapshot, OpListSnapshots, OpCheck, OpListRecoveryKeys, OpRetention:
		return true
	default:
		return false
	}
}
