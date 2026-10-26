//go:build integration

// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/restic/restic/internal/ui/restore"
	"golang.org/x/sys/unix"
)

type rootedTestFixture struct {
	base, repo, source, state, workspace string
	first, second                        string
	password, recovery                   []byte
	cfg                                  s3Config
	peer                                 *s3MemoryPeer
	remote                               bool
}

func rootedTestXattrName() string {
	if runtime.GOOS == "darwin" {
		return "com.solstone.rooted"
	}
	return "user.solstone.rooted"
}

func assertRootedTestXattr(t *testing.T, path, expected string) {
	t.Helper()
	value := make([]byte, 256)
	n, err := unix.Getxattr(path, rootedTestXattrName(), value)
	if err != nil || string(value[:n]) != expected {
		t.Fatalf("actual restored xattr mismatch at %s: %v", path, err)
	}
}

func (f *rootedTestFixture) call(t *testing.T, operation int32, password []byte) bridgeResult {
	t.Helper()
	req := bridgeRequest{operation: operation, repoPath: f.repo, password: append([]byte(nil), password...)}
	if operation == OpInit || operation == OpAddRecoveryKey {
		req.recoveryPassword = append([]byte(nil), f.recovery...)
	}
	if operation == OpSnapshot {
		req.sourcePath = f.source
	}
	if f.remote {
		id, h := s3TestStartOperation(t, f.cfg, f.state, f.workspace, f.peer, req)
		return s3TestWaitOperation(t, id, h)
	}
	if err := pinRequestPaths(&req); err != nil {
		t.Fatal(err)
	}
	if operation == OpSnapshot {
		if err := pinMetadataWorkspace(&req, f.workspace); err != nil {
			_ = rejectBridgeCall(&req, StatusInvalidArgument)
			t.Fatal(err)
		}
	}
	id, status := startBridgeCall(req)
	if status != StatusOk {
		t.Fatal(status)
	}
	handlesMu.Lock()
	h := handles[id]
	handlesMu.Unlock()
	return s3TestWaitOperation(t, id, h)
}

func newRootedTestFixture(t *testing.T, remote bool) *rootedTestFixture {
	t.Helper()
	base, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	f := &rootedTestFixture{base: base, repo: filepath.Join(base, "repo"), source: filepath.Join(base, "journal"),
		state: s3OwnerTestDirectory(t), workspace: s3OwnerTestDirectory(t), password: []byte("rooted-daily-synthetic"), recovery: []byte("rooted-recovery-distinct"), cfg: s3TestConfig(), remote: remote}
	f.peer = &s3MemoryPeer{cfg: f.cfg, objects: make(map[string][]byte)}
	for _, p := range []string{f.repo, f.source, filepath.Join(f.source, "nested")} {
		if err := os.Mkdir(p, 0700); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.WriteFile(filepath.Join(f.source, "owner.txt"), []byte("original record"), 0640); err != nil {
		t.Fatal(err)
	}
	if err := os.Link(filepath.Join(f.source, "owner.txt"), filepath.Join(f.source, "owner-alias")); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink("owner.txt", filepath.Join(f.source, "owner-link")); err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{rootedRestoreLock, "nested/" + rootedRestoreLock} {
		if err := os.WriteFile(filepath.Join(f.source, name), []byte("snapshot "+name), 0600); err != nil {
			t.Fatal(err)
		}
	}
	if err := unix.Setxattr(filepath.Join(f.source, "owner.txt"), rootedTestXattrName(), []byte("owner metadata"), 0); err != nil {
		t.Fatal(err)
	}
	stamp := time.Unix(1700000000, 123456789)
	if err := os.Chtimes(filepath.Join(f.source, "owner.txt"), stamp, stamp); err != nil {
		t.Fatal(err)
	}
	if r := f.call(t, OpInit, f.password); r.status != StatusOk {
		t.Fatal("init", r.status, r.diagnostic)
	}
	if r := f.call(t, OpAddRecoveryKey, f.password); r.status != StatusOk {
		t.Fatal("add recovery", r.status, r.diagnostic)
	}
	if r := f.call(t, OpOpen, f.recovery); r.status != StatusOk {
		t.Fatal("independent recovery", r.status, r.diagnostic)
	}
	r := f.call(t, OpSnapshot, f.password)
	if r.status != StatusOk {
		t.Fatal("original snapshot", r.status, r.diagnostic)
	}
	f.first = r.snapshotID
	if err := os.WriteFile(filepath.Join(f.source, "owner.txt"), []byte("changed record"), 0640); err != nil {
		t.Fatal(err)
	}
	if err := os.Chtimes(filepath.Join(f.source, "owner.txt"), stamp, stamp); err != nil {
		t.Fatal(err)
	}
	r = f.call(t, OpSnapshot, f.password)
	if r.status != StatusOk {
		t.Fatal("changed snapshot", r.status, r.diagnostic)
	}
	f.second = r.snapshotID
	if f.first == f.second {
		t.Fatal("actual snapshots not distinct")
	}
	if err := os.Rename(f.source, f.source+"-retained"); err != nil {
		t.Fatal(err)
	}
	return f
}

func rootedTestRequest(t *testing.T, f *rootedTestFixture, target, snapshot string, password []byte) bridgeRequest {
	t.Helper()
	borrowed, err := os.Open(target)
	if err != nil {
		t.Fatal(err)
	}
	r, status := newRootedRestore(int(borrowed.Fd()), "journal")
	_ = borrowed.Close() // The new operation must own its independent custody.
	if status != StatusOk {
		t.Fatal("rooted descriptor admission", status)
	}
	return bridgeRequest{operation: OpRestore, repoPath: f.repo, restorePath: target, snapshotID: snapshot, password: append([]byte(nil), password...), rootedRestore: r}
}

func rootedTestStart(t *testing.T, f *rootedTestFixture, req bridgeRequest) (uint64, *callHandle) {
	t.Helper()
	if f.remote {
		return s3TestStartOperation(t, f.cfg, f.state, f.workspace, f.peer, req)
	}
	if err := pinRequestPaths(&req); err != nil {
		_ = rejectBridgeCall(&req, StatusInvalidArgument)
		t.Fatal(err)
	}
	id, status := startBridgeCall(req)
	if status != StatusOk {
		t.Fatal(status)
	}
	handlesMu.Lock()
	h := handles[id]
	handlesMu.Unlock()
	t.Cleanup(func() { _ = cancelCall(id); <-h.doneCh; _ = freeCall(id) })
	return id, h
}

func rootedTestWait(t *testing.T, id uint64, h *callHandle) (bridgeResult, bool, []byte) {
	t.Helper()
	select {
	case <-h.doneCh:
	case <-time.After(45 * time.Second):
		t.Fatal("actual rooted worker did not drain")
	}
	h.mu.Lock()
	result := h.result
	h.mu.Unlock()
	var mutated bool
	if status := withRootedRestoreOutcome(id, func(admitted, may bool) int32 {
		if !admitted {
			t.Error("missing actual target admission")
		}
		mutated = may
		return StatusOk
	}); status != StatusOk {
		t.Fatal("outcome", status)
	}
	var metadata []byte
	status := withResultMetadata(id, func(value []byte) int32 { metadata = append([]byte(nil), value...); return StatusOk })
	if result.status == StatusOk && status != StatusOk {
		t.Fatal("actual successful summary unavailable", status)
	}
	if result.status != StatusOk && (status == StatusOk || len(metadata) != 0) {
		t.Fatal("failed restore exposed successful metadata")
	}
	if freeCall(id) != StatusOk {
		t.Fatal("drained handle did not free")
	}
	if status := withRootedRestoreOutcome(id, func(bool, bool) int32 { return StatusOk }); status != StatusInvalidHandle {
		t.Fatal("freed outcome admitted", status)
	}
	return result, mutated, metadata
}

func TestRootedRestoreEncryptedLocalAndS3ExistingTargets(t *testing.T) {
	for _, remote := range []bool{false, true} {
		t.Run(map[bool]string{false: "local", true: "S3"}[remote], func(t *testing.T) {
			f := newRootedTestFixture(t, remote)
			for _, snapshot := range []struct{ id, bytes string }{{f.first, "original record"}, {f.second, "changed record"}} {
				target := filepath.Join(f.base, "target-"+snapshot.id[:8])
				if err := os.Mkdir(target, 0700); err != nil {
					t.Fatal(err)
				}
				if err := os.WriteFile(filepath.Join(target, "sentinel"), []byte("unrelated"), 0600); err != nil {
					t.Fatal(err)
				}
				lock := filepath.Join(target, rootedRestoreLock)
				if err := os.WriteFile(lock, []byte("retained lease"), 0600); err != nil {
					t.Fatal(err)
				}
				lockInfo, _ := os.Stat(lock)
				for repeat := 0; repeat < 2; repeat++ {
					id, h := rootedTestStart(t, f, rootedTestRequest(t, f, target, snapshot.id, f.recovery))
					r, mutated, metadata := rootedTestWait(t, id, h)
					if r.status != StatusOk || !mutated {
						t.Fatal("actual existing-root restore", r.status, r.diagnostic, mutated)
					}
					var actual struct {
						Kind  string        `json:"kind"`
						State restore.State `json:"state"`
					}
					if json.Unmarshal(metadata, &actual) != nil || actual.Kind != "existing-root-restore" || actual.State.FilesTotal == 0 || actual.State.AllBytesWritten == 0 {
						t.Fatal("missing actual SDK progress", string(metadata))
					}
					if repeat == 1 && actual.State.FilesSkipped == 0 {
						t.Fatal("actual repeated restore skipped no matching entries")
					}
					if value, _ := os.ReadFile(filepath.Join(target, "owner.txt")); string(value) != snapshot.bytes {
						t.Fatal("wrong selected snapshot bytes")
					}
					if _, err := os.Lstat(filepath.Join(target, "journal")); !errors.Is(err, os.ErrNotExist) {
						t.Fatal("extra source basename appeared")
					}
					inside, _ := os.Stat(filepath.Join(target, "owner.txt"))
					alias, _ := os.Stat(filepath.Join(target, "owner-alias"))
					if !os.SameFile(inside, alias) || inside.Mode().Perm() != 0640 || !inside.ModTime().Equal(time.Unix(1700000000, 123456789)) {
						t.Fatal("actual hardlinks/mode/mtime not preserved")
					}
					assertRootedTestXattr(t, filepath.Join(target, "owner.txt"), "owner metadata")
					assertRootedTestXattr(t, filepath.Join(target, "owner-alias"), "owner metadata")
					link, err := os.Readlink(filepath.Join(target, "owner-link"))
					if err != nil || link != "owner.txt" {
						t.Fatal("symlink not preserved")
					}
					if value, _ := os.ReadFile(filepath.Join(target, "sentinel")); string(value) != "unrelated" {
						t.Fatal("unrelated data changed")
					}
					if value, _ := os.ReadFile(lock); string(value) != "retained lease" {
						t.Fatal("root lease overwritten")
					}
					after, _ := os.Stat(lock)
					if !os.SameFile(lockInfo, after) {
						t.Fatal("lease inode replaced")
					}
					if value, _ := os.ReadFile(filepath.Join(target, "nested", rootedRestoreLock)); string(value) != "snapshot nested/"+rootedRestoreLock {
						t.Fatal("nested same-basename owner entry excluded")
					}
				}
			}
		})
	}
}

func TestRootedRestoreDetachesExistingOutsideReadonlyHardlink(t *testing.T) {
	f := newRootedTestFixture(t, false)
	target := filepath.Join(f.base, "target")
	if err := os.Mkdir(target, 0700); err != nil {
		t.Fatal(err)
	}
	outside := filepath.Join(f.base, "outside")
	if err := os.WriteFile(outside, []byte("original record"), 0644); err != nil {
		t.Fatal(err)
	}
	if err := unix.Setxattr(outside, rootedTestXattrName(), []byte("outside metadata"), 0); err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(outside, 0444); err != nil {
		t.Fatal(err)
	}
	stamp := time.Unix(1600000000, 456789123)
	if err := os.Chtimes(outside, stamp, stamp); err != nil {
		t.Fatal(err)
	}
	if err := os.Link(outside, filepath.Join(target, "owner-alias")); err != nil {
		t.Fatal(err)
	}
	before, _ := os.Stat(outside)
	id, h := rootedTestStart(t, f, rootedTestRequest(t, f, target, f.first, f.password))
	r, _, _ := rootedTestWait(t, id, h)
	if r.status != StatusOk {
		t.Fatal("safe actual detachment", r.status, r.diagnostic)
	}
	after, _ := os.Stat(outside)
	value, _ := os.ReadFile(outside)
	if !os.SameFile(before, after) || before.Mode() != after.Mode() || !before.ModTime().Equal(after.ModTime()) || string(value) != "original record" {
		t.Fatal("outside inode changed")
	}
	assertRootedTestXattr(t, outside, "outside metadata")
	assertRootedTestXattr(t, filepath.Join(target, "owner-alias"), "owner metadata")
	inside, _ := os.Stat(filepath.Join(target, "owner-alias"))
	if os.SameFile(after, inside) {
		t.Fatal("outside link was retained")
	}
}

func TestRootedRestoreFailureOutcomeAndLeaseUntilActualDrain(t *testing.T) {
	f := newRootedTestFixture(t, false)
	target := filepath.Join(f.base, "target")
	if err := os.Mkdir(target, 0700); err != nil {
		t.Fatal(err)
	}
	// Wrong credentials are a real terminal refusal with no restore mutation.
	id, h := rootedTestStart(t, f, rootedTestRequest(t, f, target, f.first, []byte("wrong key")))
	r, mutated, _ := rootedTestWait(t, id, h)
	if r.status == StatusOk || mutated {
		t.Fatal("wrong-key target mutation outcome", r.status, mutated)
	}
	entered, release := make(chan struct{}), make(chan struct{})
	var once sync.Once
	testGate = func(stage string, ctx context.Context) error {
		if stage == "rooted-restore-before-write" {
			once.Do(func() { close(entered) })
			<-release
		}
		return nil
	}
	t.Cleanup(func() { testGate = nil })
	id, h = rootedTestStart(t, f, rootedTestRequest(t, f, target, f.first, f.password))
	select {
	case <-entered:
	case <-time.After(15 * time.Second):
		t.Fatal("actual writer barrier missing")
	}
	if freeCall(id) != StatusBusy || withRootedRestoreOutcome(id, func(bool, bool) int32 { return StatusOk }) != StatusBusy {
		t.Fatal("live native work appeared drained")
	}
	replacement := rootedTestRequest(t, f, target, f.second, f.password)
	otherRepo := filepath.Join(f.base, "other-repository")
	if err := os.Mkdir(otherRepo, 0700); err != nil {
		t.Fatal(err)
	}
	replacement.repoPath = otherRepo
	if err := pinRequestPaths(&replacement); !errors.Is(err, errS3OwnerBusy) {
		_ = rejectBridgeCall(&replacement, StatusInvalidArgument)
		t.Fatalf("same-target replacement admitted: %v", err)
	}
	_ = rejectBridgeCall(&replacement, StatusBusy)
	lock, err := os.OpenFile(filepath.Join(target, rootedRestoreLock), os.O_RDWR, 0)
	if err != nil {
		t.Fatal(err)
	}
	if err := unix.Flock(int(lock.Fd()), unix.LOCK_EX|unix.LOCK_NB); !errors.Is(err, unix.EWOULDBLOCK) && !errors.Is(err, unix.EAGAIN) {
		t.Fatal("actual independently opened lease not held", err)
	}
	_ = lock.Close()
	childCtx, childCancel := context.WithTimeout(context.Background(), 20*time.Second)
	child := exec.CommandContext(childCtx, os.Args[0], "-test.run=^TestRootedRestoreLeaseProcessHelper$")
	child.Env = append(os.Environ(), "SOLSTONE_ROOTED_TEST_LEASE="+filepath.Join(target, rootedRestoreLock))
	childOutput, childErr := child.CombinedOutput()
	childCancel()
	if childErr != nil {
		t.Fatal("separate cooperating process did not observe actual lease", childErr, string(childOutput))
	}
	if cancelCall(id) != StatusOk || freeCall(id) != StatusBusy {
		t.Fatal("cancellation released live native work")
	}
	close(release)
	r, mutated, _ = rootedTestWait(t, id, h)
	testGate = nil
	if r.status == StatusOk || mutated {
		t.Fatal("cancel-before-mutation outcome", r.status, mutated)
	}
	id, h = rootedTestStart(t, f, rootedTestRequest(t, f, target, f.first, f.password))
	r, _, _ = rootedTestWait(t, id, h)
	if r.status != StatusOk {
		t.Fatal("same-process retry after actual drain", r.status, r.diagnostic)
	}
}

func TestRootedRestoreLeaseProcessHelper(t *testing.T) {
	path := os.Getenv("SOLSTONE_ROOTED_TEST_LEASE")
	if path == "" {
		t.Skip("explicit subprocess fixture only")
	}
	file, err := os.OpenFile(path, os.O_RDWR, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()
	if err := unix.Flock(int(file.Fd()), unix.LOCK_EX|unix.LOCK_NB); !errors.Is(err, unix.EWOULDBLOCK) && !errors.Is(err, unix.EAGAIN) {
		t.Fatal("another process acquired the live original lock", err)
	}
}

func TestRootedRestoreMatchingMetadataOnlyNamedRootDriftRefuses(t *testing.T) {
	base, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	repo, source, target := filepath.Join(base, "repo"), filepath.Join(base, "journal"), filepath.Join(base, "target")
	for _, name := range []string{repo, source, target} {
		if err := os.Mkdir(name, 0700); err != nil {
			t.Fatal(err)
		}
	}
	content := []byte("matching ordinary file")
	if err := os.WriteFile(filepath.Join(source, "owner"), content, 0640); err != nil {
		t.Fatal(err)
	}
	password := []byte("metadata-only-rooted-synthetic")
	initReq := bridgeRequest{operation: OpInit, repoPath: repo, password: password}
	if r, err := executeInit(context.Background(), initReq, func() {}); err != nil || r.status != StatusOk {
		t.Fatal("init", err)
	}
	r, err := executeSnapshot(context.Background(), bridgeRequest{operation: OpSnapshot, repoPath: repo, sourcePath: source, password: password}, func() {})
	if err != nil || r.status != StatusOk {
		t.Fatal("snapshot", err)
	}
	if err := os.Rename(source, source+"-retained"); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(target, "owner"), content, 0600); err != nil {
		t.Fatal(err)
	}
	before, _ := os.Stat(filepath.Join(target, "owner"))
	f := &rootedTestFixture{repo: repo}
	var reached bool
	testGate = func(stage string, _ context.Context) error {
		if stage == "rooted-restore-before-write" && !reached {
			reached = true
			if err := os.Rename(target, target+"-retained"); err != nil {
				return err
			}
			return os.Mkdir(target, 0700)
		}
		return nil
	}
	t.Cleanup(func() { testGate = nil })
	id, h := rootedTestStart(t, f, rootedTestRequest(t, f, target, r.snapshotID, password))
	actual, mutated, _ := rootedTestWait(t, id, h)
	testGate = nil
	if !reached || actual.status == StatusOk || mutated {
		t.Fatal("metadata-only root drift did not refuse before mutation", actual.status, mutated)
	}
	after, err := os.Stat(filepath.Join(target+"-retained", "owner"))
	if err != nil || before.Mode() != after.Mode() || !before.ModTime().Equal(after.ModTime()) {
		t.Fatal("metadata guard changed retained owner inode", err)
	}
	entries, err := os.ReadDir(target)
	if err != nil || len(entries) != 0 {
		t.Fatal("new named root received metadata/data", err)
	}
}

func TestRootedRestoreMetadataOnlyDriftAndNonCancellationPartialFailure(t *testing.T) {
	f := newRootedTestFixture(t, false)
	target := filepath.Join(f.base, "target")
	if err := os.Mkdir(target, 0700); err != nil {
		t.Fatal(err)
	}
	id, h := rootedTestStart(t, f, rootedTestRequest(t, f, target, f.first, f.password))
	r, _, _ := rootedTestWait(t, id, h)
	if r.status != StatusOk {
		t.Fatal(r.status)
	}
	// Observe an actual first restored entry before injecting a non-cancel error.
	if err := os.Remove(filepath.Join(target, "nested", rootedRestoreLock)); err != nil {
		t.Fatal(err)
	}
	injected := errors.New("controlled non cancellation restore failure")
	var calls int
	testGate = func(stage string, _ context.Context) error {
		if stage == "rooted-restore-before-write" {
			calls++
			if data, err := os.ReadFile(filepath.Join(target, "nested", rootedRestoreLock)); err == nil && strings.HasPrefix(string(data), "snapshot nested/") {
				return injected
			}
		}
		return nil
	}
	t.Cleanup(func() { testGate = nil })
	id, h = rootedTestStart(t, f, rootedTestRequest(t, f, target, f.first, f.password))
	r, mutated, _ := rootedTestWait(t, id, h)
	testGate = nil
	if r.status == StatusOk || !mutated || calls < 2 {
		t.Fatal("non-cancellation partial failure hidden", r.status, mutated, calls)
	}
	// Root replacement after admission cannot redirect a metadata-only restore.
	var renamed bool
	testGate = func(stage string, _ context.Context) error {
		if stage == "rooted-restore-before-write" && !renamed {
			renamed = true
			if err := os.Rename(target, target+"-retained"); err != nil {
				return err
			}
			return os.Mkdir(target, 0700)
		}
		return nil
	}
	id, h = rootedTestStart(t, f, rootedTestRequest(t, f, target, f.first, f.password))
	r, _, _ = rootedTestWait(t, id, h)
	testGate = nil
	if r.status == StatusOk || !renamed {
		t.Fatal("named-root replacement became success")
	}
	entries, err := os.ReadDir(target)
	if err != nil || len(entries) != 0 {
		t.Fatal("replacement root received restore writes", err)
	}
}

func TestRootedRestorePartialWriteKeepsTheTargetObligation(t *testing.T) {
	base, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	repo := filepath.Join(base, "repo")
	source := filepath.Join(base, "journal")
	target := filepath.Join(base, "target")
	for _, path := range []string{repo, source, filepath.Join(source, "nested"), target, filepath.Join(target, "health")} {
		if err := os.MkdirAll(path, 0700); err != nil {
			t.Fatal(err)
		}
	}
	owner := []byte("original record")
	if err := os.WriteFile(filepath.Join(source, "owner.txt"), owner, 0640); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(source, "nested", "marker.txt"), []byte("later file"), 0640); err != nil {
		t.Fatal(err)
	}
	obligation := []byte("target-obligation")
	obligationPath := filepath.Join(target, "health", "whole-journal-restore.json")
	if err := os.WriteFile(obligationPath, obligation, 0640); err != nil {
		t.Fatal(err)
	}
	password := []byte("rooted-daily-synthetic")
	if r, err := executeInit(context.Background(), bridgeRequest{operation: OpInit, repoPath: repo, password: password}, func() {}); err != nil || r.status != StatusOk {
		t.Fatal("init", err, r.status)
	}
	snap, err := executeSnapshot(context.Background(), bridgeRequest{operation: OpSnapshot, repoPath: repo, sourcePath: source, password: password}, func() {})
	if err != nil || snap.status != StatusOk {
		t.Fatal("snapshot", err, snap.status)
	}
	if err := os.Rename(source, source+"-retained"); err != nil {
		t.Fatal(err)
	}
	var stopped bool
	testGate = func(stage string, _ context.Context) error {
		if stage != "rooted-restore-before-write" {
			return nil
		}
		for _, name := range []string{"owner.txt", filepath.Join("nested", "marker.txt")} {
			if _, err := os.Stat(filepath.Join(target, name)); err == nil {
				stopped = true
				return errors.New("stop after a restored file")
			}
		}
		return nil
	}
	t.Cleanup(func() { testGate = nil })
	f := &rootedTestFixture{repo: repo}
	id, h := rootedTestStart(t, f, rootedTestRequest(t, f, target, snap.snapshotID, password))
	actual, mutated, _ := rootedTestWait(t, id, h)
	testGate = nil
	if !stopped || !mutated || actual.status == StatusOk {
		t.Fatal("partial restore did not stop after a real write", stopped, mutated, actual.status)
	}
	if value, err := os.ReadFile(obligationPath); err != nil || string(value) != string(obligation) {
		t.Fatal("target obligation changed", err, value)
	}
	if value, err := os.ReadFile(filepath.Join(source+"-retained", "owner.txt")); err != nil || string(value) != string(owner) {
		t.Fatal("source snapshot bytes changed", err)
	}
}

func TestRootedRestoreSnapshotCannotReplaceTheTargetObligation(t *testing.T) {
	base, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	repo := filepath.Join(base, "repo")
	source := filepath.Join(base, "journal")
	target := filepath.Join(base, "target")
	for _, path := range []string{
		repo,
		filepath.Join(source, "health"),
		filepath.Join(source, "nested", "health"),
		filepath.Join(target, "health"),
	} {
		if err := os.MkdirAll(path, 0700); err != nil {
			t.Fatal(err)
		}
	}
	owner := []byte("original record")
	snapshotObligation := []byte("snapshot-obligation")
	nestedObligation := []byte("nested-obligation")
	targetObligation := []byte("target-obligation")
	writes := map[string][]byte{
		filepath.Join(source, "owner.txt"):                                      owner,
		filepath.Join(source, "health", "whole-journal-restore.json"):           snapshotObligation,
		filepath.Join(source, "nested", "health", "whole-journal-restore.json"): nestedObligation,
		filepath.Join(target, "health", "whole-journal-restore.json"):           targetObligation,
		filepath.Join(target, "sentinel"):                                       []byte("unrelated"),
		filepath.Join(source, "health", "!solstone-claim-00000000-0000000000000123"): []byte("foreign claim"),
		filepath.Join(target, "health", "!solstone-claim-00000000-0000000000000456"): []byte("retained local claim"),
	}
	for path, body := range writes {
		if err := os.WriteFile(path, body, 0640); err != nil {
			t.Fatal(err)
		}
	}
	password := []byte("rooted-daily-synthetic")
	if r, err := executeInit(context.Background(), bridgeRequest{operation: OpInit, repoPath: repo, password: password}, func() {}); err != nil || r.status != StatusOk {
		t.Fatal("init", err, r.status)
	}
	snap, err := executeSnapshot(context.Background(), bridgeRequest{operation: OpSnapshot, repoPath: repo, sourcePath: source, password: password}, func() {})
	if err != nil || snap.status != StatusOk {
		t.Fatal("snapshot", err, snap.status)
	}
	if err := os.Rename(source, source+"-retained"); err != nil {
		t.Fatal(err)
	}
	f := &rootedTestFixture{repo: repo}
	id, h := rootedTestStart(t, f, rootedTestRequest(t, f, target, snap.snapshotID, password))
	actual, mutated, _ := rootedTestWait(t, id, h)
	if actual.status != StatusOk || !mutated {
		t.Fatal("rooted restore", actual.status, actual.diagnostic, mutated)
	}
	if value, err := os.ReadFile(filepath.Join(target, "health", "whole-journal-restore.json")); err != nil || string(value) != string(targetObligation) {
		t.Fatal("snapshot replaced the target obligation", err, value)
	}
	if value, err := os.ReadFile(filepath.Join(target, "nested", "health", "whole-journal-restore.json")); err != nil || string(value) != string(nestedObligation) {
		t.Fatal("nested obligation path was not restored", err, value)
	}
	if _, err := os.Lstat(filepath.Join(target, "health", "!solstone-claim-00000000-0000000000000123")); !os.IsNotExist(err) {
		t.Fatal("snapshot imported a foreign local recovery claim", err)
	}
	if value, err := os.ReadFile(filepath.Join(target, "health", "!solstone-claim-00000000-0000000000000456")); err != nil || string(value) != "retained local claim" {
		t.Fatal("snapshot changed a local recovery claim", err, value)
	}
	if value, err := os.ReadFile(filepath.Join(target, "owner.txt")); err != nil || string(value) != string(owner) {
		t.Fatal("owner bytes were not restored", err, value)
	}
	if value, err := os.ReadFile(filepath.Join(target, "sentinel")); err != nil || string(value) != "unrelated" {
		t.Fatal("unrelated target data changed", err, value)
	}
	if value, err := os.ReadFile(filepath.Join(source+"-retained", "health", "whole-journal-restore.json")); err != nil || string(value) != string(snapshotObligation) {
		t.Fatal("source snapshot obligation changed", err, value)
	}
}
