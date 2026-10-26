//go:build linux || darwin || ios

// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

import (
	"errors"
	"io"
	"os"
	"strings"

	"golang.org/x/sys/unix"
)

func ownedDirectoryFile(entry *ownedCall, fd int, name string) *os.File {
	file := os.NewFile(uintptr(fd), name)
	entry.hold(file)
	return file
}

func openOwnedDirectory(path string, entry *ownedCall) (*os.File, error) {
	if entry == nil {
		return openPinnedDirectory(path)
	}
	if entry.failed() {
		return nil, errOwnedCleanup
	}
	if err := sanitizePath(path); err != nil {
		return nil, err
	}
	if testOwnedStage != nil {
		testOwnedStage("resource-open", entry)
	}
	fd, err := unix.Open("/", ownedDirectoryFlags(path == "/"), 0)
	if err != nil {
		return nil, err
	}
	current := ownedDirectoryFile(entry, fd, "versioned directory ancestor")
	components := strings.Split(strings.TrimPrefix(path, "/"), "/")
	for i, component := range components {
		if component == "" {
			continue
		}
		nextFD, openErr := unix.Openat(int(current.Fd()), component, ownedDirectoryFlags(i == len(components)-1), 0)
		var next *os.File
		if openErr == nil {
			next = ownedDirectoryFile(entry, nextFD, "versioned directory ancestor")
		}
		if openErr != nil {
			entry.scrubRejection()
		}
		closeErr := entry.close(current, "directory-open-ancestor")
		if closeErr != nil {
			return nil, closeErr
		}
		if openErr != nil {
			return nil, openErr
		}
		current = next
	}
	return current, nil
}

func adoptOwnedRoot(path string, pinned *os.File, entry *ownedCall) (*os.Root, error) {
	if entry == nil {
		return adoptRoot(path, pinned)
	}
	if entry.failed() {
		return nil, errOwnedCleanup
	}
	if testOwnedStage != nil {
		testOwnedStage("before-adopt-root", entry)
	}
	root, err := os.OpenRoot(path)
	if err != nil {
		return nil, errPinnedIdentity
	}
	entry.hold(root)
	info, statErr := root.Stat(".")
	held, heldErr := pinned.Stat()
	if statErr != nil || heldErr != nil || !os.SameFile(info, held) {
		entry.scrubRejection()
		if closeErr := entry.close(root, "adopt-rejected-root"); closeErr != nil {
			return nil, closeErr
		}
		return nil, errPinnedIdentity
	}
	return root, nil
}

type ownedAncestor struct {
	file *os.File
	stat unix.Stat_t
}

func buildOwnedAncestors(file *os.File, entry *ownedCall) (chain []ownedAncestor, err error) {
	fd, err := unix.FcntlInt(file.Fd(), unix.F_DUPFD_CLOEXEC, 0)
	if err != nil {
		return nil, err
	}
	current := ownedDirectoryFile(entry, fd, "versioned ancestry")
	for depth := 0; depth < 128; depth++ {
		var stat unix.Stat_t
		if err = unix.Fstat(int(current.Fd()), &stat); err != nil {
			return append(chain, ownedAncestor{file: current}), err
		}
		if stat.Mode&unix.S_IFMT != unix.S_IFDIR {
			return append(chain, ownedAncestor{file: current}), errPathNotDir
		}
		chain = append(chain, ownedAncestor{file: current, stat: stat})
		parentFD, openErr := unix.Openat(int(current.Fd()), "..", ownedDirectoryFlags(false), 0)
		if openErr != nil {
			return chain, openErr
		}
		parent := ownedDirectoryFile(entry, parentFD, "versioned ancestry parent")
		var parentStat unix.Stat_t
		if err = unix.Fstat(parentFD, &parentStat); err != nil {
			chain = append(chain, ownedAncestor{file: parent})
			return chain, err
		}
		if stat.Dev == parentStat.Dev && stat.Ino == parentStat.Ino {
			if err = entry.close(parent, "ancestry-root-parent"); err != nil {
				return chain, err
			}
			return chain, nil
		}
		current = parent
	}
	// The last opened parent remains actual owned custody even on bound refusal.
	entry.scrubRejection()
	if err = entry.close(current, "ancestry-bound-parent"); err != nil {
		return chain, err
	}
	return chain, errors.New("directory ancestry exceeds bound")
}
func closeOwnedAncestors(chain []ownedAncestor, entry *ownedCall) error {
	for _, dir := range chain {
		if err := entry.close(dir.file, "ancestry-chain"); err != nil {
			return err
		}
	}
	return nil
}
func validateOwnedAncestors(chain []ownedAncestor) error {
	for i, dir := range chain {
		expected := dir.stat
		if i+1 < len(chain) {
			expected = chain[i+1].stat
		}
		var actual unix.Stat_t
		if err := unix.Fstatat(int(dir.file.Fd()), "..", &actual, unix.AT_SYMLINK_NOFOLLOW); err != nil {
			return err
		}
		if actual.Dev != expected.Dev || actual.Ino != expected.Ino {
			return errPinnedIdentity
		}
	}
	return nil
}
func ownedChainContains(chain []ownedAncestor, endpoint unix.Stat_t) bool {
	for _, dir := range chain {
		if dir.stat.Dev == endpoint.Dev && dir.stat.Ino == endpoint.Ino {
			return true
		}
	}
	return false
}

func ownedDirectoriesOverlap(a, b *os.File, entry *ownedCall) (overlap bool, resultErr error) {
	if entry == nil {
		return pinnedDirectoriesOverlap(a, b)
	}
	if entry.failed() {
		return false, errOwnedCleanup
	}
	first, err := buildOwnedAncestors(a, entry)
	defer func() {
		if resultErr != nil || overlap {
			entry.scrubRejection()
		}
		resultErr = errors.Join(resultErr, closeOwnedAncestors(first, entry))
	}()
	if err != nil {
		return false, err
	}
	second, err := buildOwnedAncestors(b, entry)
	defer func() {
		if resultErr != nil || overlap {
			entry.scrubRejection()
		}
		resultErr = errors.Join(resultErr, closeOwnedAncestors(second, entry))
	}()
	if err != nil {
		return false, err
	}
	if ownedChainContains(first, second[0].stat) || ownedChainContains(second, first[0].stat) {
		return true, nil
	}
	if err := validateOwnedAncestors(first); err != nil {
		return false, err
	}
	if err := validateOwnedAncestors(second); err != nil {
		return false, err
	}
	return false, nil
}

func reserveOwnedPrivateDirectory(file *os.File, entry *ownedCall) error {
	if entry == nil {
		return reserveRootedPrivateDirectory(file)
	}
	rootedTargets.Lock()
	defer rootedTargets.Unlock()
	if len(rootedTargets.failed) != 0 {
		return errTargetAdmissionCleanup
	}
	for _, target := range rootedTargets.live {
		if overlap, err := ownedDirectoriesOverlap(file, target.target, entry); err != nil || overlap {
			if err != nil {
				return err
			}
			return errS3OwnerBusy
		}
	}
	rootedTargets.private[file] = struct{}{}
	return nil
}
func closeOwnedPrivateDirectory(file *os.File, entry *ownedCall, stage string) error {
	if entry == nil {
		return closeRootedPrivateDirectory(file)
	}
	rootedTargets.Lock()
	defer rootedTargets.Unlock()
	if err := entry.close(file, stage); err != nil {
		return err
	}
	delete(rootedTargets.private, file)
	return nil
}

func closeOwnedRequestPaths(req *bridgeRequest) (resultErr error) {
	entry := req.owned
	entry.retain(req)
	return entry.cleanup(func() error {
		// This cohort does not admit restore. Never consume a target owner's lease.
		if req.rootedRestore != nil || req.restoreRoot != nil || req.restoreStage != nil {
			return entry.markFault()
		}
		if req.metadataRoot != nil {
			resultErr = cleanupMetadataWorkspace(req)
		}
		if entry.failed() {
			return errOwnedCleanup
		}
		if req.metadataRoot != nil {
			if err := entry.close(req.metadataRoot, "metadata-root"); err != nil {
				return err
			}
			req.metadataRoot = nil
		}
		if req.metadataFile != nil {
			if err := closeOwnedPrivateDirectory(req.metadataFile, entry, "metadata-directory"); err != nil {
				return err
			}
			req.metadataFile = nil
		}
		if req.repoRoot != nil {
			if err := entry.close(req.repoRoot, "repository-root"); err != nil {
				return err
			}
			req.repoRoot = nil
		}
		if req.sourceRoot != nil {
			if err := entry.close(req.sourceRoot, "source-root"); err != nil {
				return err
			}
			req.sourceRoot = nil
		}
		if req.archiveTarget != nil {
			if err := req.archiveTarget.Close(); err != nil {
				resultErr = errors.Join(resultErr, err)
			}
			req.archiveTarget = nil
		}
		for _, file := range req.pathFiles {
			if err := closeOwnedPrivateDirectory(file, entry, "request-directory"); err != nil {
				return err
			}
		}
		req.pathFiles = nil
		req.pathsPinned = false
		return resultErr
	})
}

// Metadata admission already restricts this namespace to one private symlink.
// Recheck that closed shape rather than invoking the generic recursive restore
// cleaner, whose descriptor lifetime belongs to the legacy restore path.
func clearOwnedMetadataDirectory(fd int, entry *ownedCall) error {
	if entry == nil {
		return clearDirectoryContents(fd)
	}
	if entry.failed() {
		return errOwnedCleanup
	}
	if err := clearFileFlags(fd); err != nil {
		return err
	}
	if err := unix.Fchmod(fd, 0700); err != nil {
		return err
	}
	listingFD, err := unix.Openat(fd, ".", unix.O_RDONLY|unix.O_DIRECTORY|unix.O_CLOEXEC, 0)
	if err != nil {
		return err
	}
	listing := ownedDirectoryFile(entry, listingFD, "versioned metadata leaf census")
	names, readErr := listing.Readdirnames(2)
	if errors.Is(readErr, io.EOF) {
		readErr = nil
	}
	closeErr := entry.close(listing, "metadata-leaf-census")
	if readErr != nil || closeErr != nil {
		return errors.Join(readErr, closeErr)
	}
	if len(names) > 1 {
		return errMetadataCleanup
	}
	var directory unix.Stat_t
	if err := unix.Fstat(fd, &directory); err != nil {
		return err
	}
	for _, name := range names {
		if !isMetadataNonce(name) {
			return errMetadataCleanup
		}
		var expected unix.Stat_t
		if err := unix.Fstatat(fd, name, &expected, unix.AT_SYMLINK_NOFOLLOW); err != nil {
			return err
		}
		if expected.Mode&unix.S_IFMT != unix.S_IFLNK || expected.Uid != uint32(os.Geteuid()) || expected.Dev != directory.Dev {
			return errMetadataCleanup
		}
		childFD, err := openCleanupEntryAt(fd, name, uint32(expected.Mode))
		if err != nil {
			return err
		}
		child := ownedDirectoryFile(entry, childFD, "versioned metadata leaf")
		var actual unix.Stat_t
		statErr := unix.Fstat(childFD, &actual)
		matches, matchErr := pinnedEntryMatches(fd, name, child)
		if statErr != nil || matchErr != nil || !matches || actual.Dev != expected.Dev || actual.Ino != expected.Ino {
			return errors.Join(errMetadataCleanup, statErr, matchErr, entry.close(child, "metadata-leaf-identity"))
		}
		flagsErr := clearFileFlags(childFD)
		closeErr := entry.close(child, "metadata-leaf-identity")
		if flagsErr != nil || closeErr != nil {
			return errors.Join(flagsErr, closeErr)
		}
		if err := unix.Unlinkat(fd, name, 0); err != nil {
			return err
		}
	}
	return nil
}
