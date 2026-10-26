// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"strings"
	"sync"
	"sync/atomic"
	"time"
	"unicode/utf8"

	"github.com/restic/restic/internal/data"
	"github.com/restic/restic/internal/repository"
	"github.com/restic/restic/internal/restic"
	"github.com/restic/restic/internal/restorer"
	"github.com/restic/restic/internal/ui/progress"
	"github.com/restic/restic/internal/ui/restore"
	"golang.org/x/sys/unix"
)

const rootedRestoreLock = "backup-activity.lock"
const wholeJournalRestoreFile = "health/whole-journal-restore.json"

type rootedTargetIdentity struct{ device, inode uint64 }

var rootedTargets = struct {
	sync.Mutex
	live    map[rootedTargetIdentity]*rootedRestore
	private map[*os.File]struct{}
	owners  map[uint64]*rootedTargetAdmission
	failed  map[*rootedRestore]struct{}
}{live: make(map[rootedTargetIdentity]*rootedRestore), private: make(map[*os.File]struct{}),
	owners: make(map[uint64]*rootedTargetAdmission), failed: make(map[*rootedRestore]struct{})}

// Both admission orders use this mutex. Checking live private custody and
// reserving a restore target are one decision, including ancestor aliases.
// Legacy private operations retain their old mutual concurrency policy; only
// overlap with the new existing-root writer introduces a refusal.
func reserveRootedPrivateDirectory(file *os.File) error {
	rootedTargets.Lock()
	defer rootedTargets.Unlock()
	if len(rootedTargets.failed) != 0 {
		return errTargetAdmissionCleanup
	}
	for _, target := range rootedTargets.live {
		if overlap, err := pinnedDirectoriesOverlap(file, target.target); err != nil || overlap {
			if err != nil {
				return err
			}
			return errS3OwnerBusy
		}
	}
	rootedTargets.private[file] = struct{}{}
	return nil
}

func closeRootedPrivateDirectory(file *os.File) error {
	rootedTargets.Lock()
	defer rootedTargets.Unlock()
	delete(rootedTargets.private, file)
	return file.Close()
}

// This object owns admission; the terminal handle retains its small outcome
// after descriptor cleanup. It never unlocks a caller's shared description.
type rootedRestore struct {
	selector  string
	target    *os.File
	lease     *os.File
	identity  rootedTargetIdentity
	reserved  bool
	admitted  bool
	mayMutate atomic.Bool
	// A borrowed operation never closes its owner's descriptors. Its borrow
	// remains live through terminal cleanup and is released only by Free.
	borrowed       *rootedTargetAdmission
	borrowReleased bool  // rootedTargets mutex
	closeFailure   error // rootedTargets mutex; never retry an ambiguous close
}

func validateRootedSelector(value string) bool {
	if len(value) == 0 || len(value) > maxPathLen || !utf8.ValidString(value) || strings.ContainsAny(value, "\\\x00\r\n") {
		return false
	}
	parts := strings.Split(strings.TrimPrefix(value, "/"), "/")
	if len(parts) > 64 {
		return false
	}
	for _, part := range parts {
		if part == "" || part == "." || part == ".." {
			return false
		}
	}
	return true
}

func newRootedRestore(fd int, selector string) (*rootedRestore, int32) {
	if fd < 0 || !validateRootedSelector(selector) {
		return nil, StatusInvalidArgument
	}
	rooted, status := duplicateRootedTarget(fd)
	if status != StatusOk {
		if rooted != nil {
			_ = rooted.close() // failed cleanup remains in process-owned refusal custody
		}
		return nil, status
	}
	rooted.selector = selector
	return rooted, StatusOk
}

// The caller owns any returned object, including on validation failure.
func duplicateRootedTarget(fd int) (*rootedRestore, int32) {
	if fd < 0 {
		return nil, StatusInvalidArgument
	}
	owned, err := unix.FcntlInt(uintptr(fd), unix.F_DUPFD_CLOEXEC, 0)
	if err != nil {
		return nil, StatusInvalidArgument
	}
	rooted := &rootedRestore{target: os.NewFile(uintptr(owned), "restore target")}
	var stat unix.Stat_t
	if unix.Fstat(owned, &stat) != nil || stat.Mode&unix.S_IFMT != unix.S_IFDIR || stat.Uid != uint32(os.Geteuid()) {
		return rooted, StatusInvalidArgument
	}
	rooted.identity = rootedTargetIdentity{uint64(stat.Dev), stat.Ino}
	return rooted, StatusOk
}

func (r *rootedRestore) close() error {
	if r.borrowed != nil {
		return nil
	}
	rootedTargets.Lock()
	defer rootedTargets.Unlock()
	return r.closeLocked()
}

func (r *rootedRestore) failCloseLocked(err error) error {
	r.closeFailure = errors.Join(r.closeFailure, err)
	rootedTargets.failed[r] = struct{}{}
	return r.closeFailure
}

func (r *rootedRestore) closeLocked() error {
	if r.closeFailure != nil {
		return r.closeFailure
	}
	if r.lease != nil {
		file := r.lease
		r.lease = nil
		if err := closeTargetAdmissionResource("lease", file); err != nil {
			return r.failCloseLocked(err)
		}
	}
	if r.target != nil {
		file := r.target
		r.target = nil
		if err := closeTargetAdmissionResource("target", file); err != nil {
			return r.failCloseLocked(err)
		}
	}
	if r.reserved {
		if rootedTargets.live[r.identity] == r {
			delete(rootedTargets.live, r.identity)
		}
		r.reserved = false
	}
	return nil
}

func pinRootedRestore(req *bridgeRequest, protected []*os.File) error {
	r := req.rootedRestore
	if r == nil || r.target == nil || !utf8.ValidString(req.restorePath) || validateExistingDir(req.restorePath) != nil {
		return errPinnedIdentity
	}
	for _, other := range protected {
		if overlap, err := pinnedDirectoriesOverlap(other, r.target); err != nil || overlap {
			if err != nil {
				return err
			}
			return errPathOverlap
		}
	}
	root, err := adoptRoot(req.restorePath, r.target)
	if err != nil {
		return err
	}
	req.restoreRoot = root
	if r.borrowed != nil {
		rootedTargets.Lock()
		owner := r.borrowed
		valid := rootedTargets.owners[owner.id] == owner && owner.borrow == r && !r.borrowReleased &&
			owner.rooted.closeFailure == nil && req.restorePath == owner.path
		rootedTargets.Unlock()
		if !valid {
			return errPinnedIdentity
		}
		if err := recheckRootedRestore(req); err != nil {
			return err
		}
		r.admitted, req.pathsPinned = true, true
		return nil
	}
	rootedTargets.Lock()
	if len(rootedTargets.failed) != 0 {
		rootedTargets.Unlock()
		return errTargetAdmissionCleanup
	}
	for _, target := range rootedTargets.live {
		if overlap, err := pinnedDirectoriesOverlap(target.target, r.target); err != nil || overlap {
			rootedTargets.Unlock()
			if err != nil {
				return err
			}
			return errS3OwnerBusy
		}
	}
	for private := range rootedTargets.private {
		if overlap, err := pinnedDirectoriesOverlap(private, r.target); err != nil || overlap {
			rootedTargets.Unlock()
			if err != nil {
				return err
			}
			return errS3OwnerBusy
		}
	}
	rootedTargets.live[r.identity], r.reserved = r, true
	rootedTargets.Unlock()
	fd := int(r.target.Fd())
	var before unix.Stat_t
	statErr := unix.Fstatat(fd, rootedRestoreLock, &before, unix.AT_SYMLINK_NOFOLLOW)
	if statErr != nil && !errors.Is(statErr, unix.ENOENT) {
		return errPinnedIdentity
	}
	if statErr == nil && (before.Mode&unix.S_IFMT != unix.S_IFREG || before.Nlink != 1 || before.Uid != uint32(os.Geteuid())) {
		return errPinnedIdentity
	}
	flags := unix.O_RDWR | unix.O_NOFOLLOW | unix.O_CLOEXEC | unix.O_NONBLOCK
	if errors.Is(statErr, unix.ENOENT) {
		flags |= unix.O_CREAT | unix.O_EXCL
	}
	lockFD, err := unix.Openat(fd, rootedRestoreLock, flags, 0600)
	if err != nil {
		return err
	}
	r.lease = os.NewFile(uintptr(lockFD), "restore activity lease")
	var lockStat unix.Stat_t
	if unix.Fstat(lockFD, &lockStat) != nil || lockStat.Mode&unix.S_IFMT != unix.S_IFREG || lockStat.Nlink != 1 || lockStat.Uid != uint32(os.Geteuid()) ||
		(statErr == nil && (lockStat.Dev != before.Dev || lockStat.Ino != before.Ino)) {
		return errPinnedIdentity
	}
	if err := unix.Flock(lockFD, unix.LOCK_EX|unix.LOCK_NB); err != nil {
		return err
	}
	if err := recheckRootedRestore(req); err != nil {
		return err
	}
	r.admitted = true
	req.pathsPinned = true
	return nil
}

func recheckRootedRestore(req *bridgeRequest) error {
	r := req.rootedRestore
	if r == nil || r.target == nil || r.lease == nil {
		return errPinnedIdentity
	}
	named, err := openPinnedDirectory(req.restorePath)
	if err != nil {
		return errPinnedIdentity
	}
	defer named.Close()
	if same, err := sameDirectory(named, r.target); err != nil || !same {
		return errPinnedIdentity
	}
	if same, err := pinnedEntryMatches(int(r.target.Fd()), rootedRestoreLock, r.lease); err != nil || !same {
		return errPinnedIdentity
	}
	var stat unix.Stat_t
	if unix.Fstat(int(r.lease.Fd()), &stat) != nil || stat.Mode&unix.S_IFMT != unix.S_IFREG || stat.Nlink != 1 || stat.Uid != uint32(os.Geteuid()) {
		return errPinnedIdentity
	}
	return nil
}

type rootedRestoreProgress struct {
	silentRestoreProgressPrinter
	mu       sync.Mutex
	state    restore.State
	finished bool
}

func (p *rootedRestoreProgress) Finish(state restore.State, _ time.Duration) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.state, p.finished = state, true
}

func (p *rootedRestoreProgress) metadata() ([]byte, int32) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if !p.finished {
		return nil, StatusFailed
	}
	encoded, err := json.Marshal(struct {
		Schema string        `json:"schema"`
		Kind   string        `json:"kind"`
		State  restore.State `json:"state"`
	}{metadataSchema, "existing-root-restore", p.state})
	if err != nil || len(encoded) > maxResultMetadata {
		return nil, StatusInternal
	}
	return encoded, StatusOk
}

func executeRootedRestore(ctx context.Context, req *bridgeRequest, setWrote func()) (bridgeResult, error) {
	failed := func(err error) (bridgeResult, error) {
		if ctx.Err() != nil {
			return bridgeResult{status: StatusCancelled, diagnostic: DiagCancelled}, err
		}
		return refusalResult(err, scrubError(err, *req)), err
	}
	if req.rootedRestore == nil || !req.rootedRestore.admitted || !req.pathsPinned {
		return failed(errPinnedIdentity)
	}
	pw, clear := passwordString(req.password)
	defer clear()
	be, err := operationBackend(ctx, *req, false)
	if err != nil {
		return failed(err)
	}
	defer be.Close()
	repo, err := repository.New(guardedBackend{be}, repository.Options{})
	if err != nil {
		return failed(err)
	}
	if _, err := safeScanKey(ctx, repo, pw); err != nil {
		return keyRefusal(ctx, be, err), err
	}
	unlock, lockCtx, err := lockRepo(ctx, repo, false)
	if err != nil {
		return failed(err)
	}
	defer unlock()
	// Past the key and the lock, an error is never a password or a missing
	// repository, whatever its text says; restore may already have written.
	failed = func(err error) (bridgeResult, error) {
		if ctx.Err() != nil {
			return bridgeResult{status: StatusCancelled, diagnostic: DiagCancelled}, err
		}
		if errors.Is(err, errResourceLimit) {
			return refusalResult(err, DiagIoFailed), err
		}
		if strings.Contains(err.Error(), "ciphertext verification failed") {
			return bridgeResult{status: StatusFailed, diagnostic: DiagRepoCorrupt}, err
		}
		return bridgeResult{status: StatusFailed, diagnostic: DiagIoFailed}, err
	}
	if err := repo.LoadIndex(lockCtx, &progress.NoopPrinter{}); err != nil {
		return failed(err)
	}
	id, err := restic.ParseID(req.snapshotID)
	if err != nil {
		return failed(err)
	}
	snapshot, err := data.LoadSnapshot(lockCtx, repo, id)
	if err != nil {
		return failed(err)
	}
	if snapshot.Tree == nil || len(snapshot.Paths) != 1 || !validateRootedSelector(snapshot.Paths[0]) ||
		strings.TrimPrefix(snapshot.Paths[0], "/") != strings.TrimPrefix(req.rootedRestore.selector, "/") {
		return bridgeResult{status: StatusInvalidArgument, diagnostic: DiagInvalidArgument}, errPinnedIdentity
	}
	tree, err := data.FindTreeDirectory(lockCtx, repo, snapshot.Tree, req.rootedRestore.selector)
	if err != nil {
		return failed(err)
	}
	selected := *snapshot
	selected.Tree = tree
	printer := &rootedRestoreProgress{}
	prog := restore.NewProgress(printer, time.Hour)
	var finishOnce sync.Once
	finish := func() { finishOnce.Do(prog.Finish) }
	defer finish()
	res := restorer.NewRestorer(repo, &selected, restorer.Options{Progress: prog, ExistingRoot: true})
	res.SelectFilter = func(item string, _ bool) (bool, bool) {
		if item == "/"+rootedRestoreLock || item == rootedRestoreLock ||
			item == "/"+wholeJournalRestoreFile || item == wholeJournalRestoreFile {
			return false, false
		}
		if strings.HasPrefix(strings.TrimPrefix(item, "/"), "health/!solstone-claim-00000000-") {
			return false, false
		}
		return true, true
	}
	var mutationOnce sync.Once
	res.BeforeWrite = func() error {
		if err := lockCtx.Err(); err != nil {
			return err
		}
		if err := recheckRootedRestore(req); err != nil {
			return err
		}
		if testGate != nil {
			if err := testGate("rooted-restore-before-write", lockCtx); err != nil {
				return err
			}
		}
		// Recheck after a test barrier too; a held syscall is never detached.
		if err := lockCtx.Err(); err != nil {
			return err
		}
		if err := recheckRootedRestore(req); err != nil {
			return err
		}
		req.rootedRestore.mayMutate.Store(true)
		mutationOnce.Do(setWrote)
		return nil
	}
	if _, err := res.RestoreToRoot(lockCtx, req.restoreRoot); err != nil {
		return failed(err)
	}
	if err := lockCtx.Err(); err != nil {
		return failed(err)
	}
	if err := recheckRootedRestore(req); err != nil {
		return failed(err)
	}
	finish()
	result := bridgeResult{status: StatusOk}
	result.metadata, result.metadataStatus = printer.metadata()
	return result, nil
}

func withRootedRestoreOutcome(id uint64, copyResult func(bool, bool) int32) int32 {
	handlesMu.Lock()
	h := handles[id]
	handlesMu.Unlock()
	if h == nil {
		return StatusInvalidHandle
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.state == handleFreed {
		return StatusInvalidHandle
	}
	select {
	case <-h.doneCh:
	default:
		return StatusBusy
	}
	if h.rootedRestore == nil {
		return StatusInvalidArgument
	}
	return copyResult(h.rootedRestore.admitted, h.rootedRestore.mayMutate.Load())
}
