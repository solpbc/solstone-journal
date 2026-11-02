//go:build integration

// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/klauspost/compress/zstd"
	"github.com/restic/restic/internal/backend"
	"github.com/restic/restic/internal/crypto"
	"github.com/restic/restic/internal/repository"
	"github.com/restic/restic/internal/restic"
)

// Controlled construction is private to tests. Production C admission installs
// no factory and always uses the fixed linked Apple implementation.
func s3TestStartOperation(t *testing.T, cfg s3Config, state, workspace string, transport http.RoundTripper, req bridgeRequest) (uint64, *callHandle) {
	t.Helper()
	owner, err := newS3Owner(state, cfg)
	if err != nil {
		t.Fatal("operation owner refused", err)
	}
	return s3TestStartOwnedOperation(t, cfg, owner, workspace, transport, req)
}

func s3TestStartOwnedOperation(t *testing.T, cfg s3Config, owner *s3Owner, workspace string, transport http.RoundTripper, req bridgeRequest) (uint64, *callHandle) {
	t.Helper()
	// The worker owns and clears input buffers. Preserve reusable fixture custody.
	req.password = append([]byte(nil), req.password...)
	req.recoveryPassword = append([]byte(nil), req.recoveryPassword...)
	req.remote = &s3RemoteRequest{cfg: cfg, owner: owner, factory: func(ctx context.Context, cfg s3Config) (*s3Backend, error) {
		owned, cancel := context.WithCancel(ctx)
		return &s3Backend{cfg: cfg, transport: transport, ctx: owned, cancel: cancel}, nil
	}}
	if err := pinDataPaths(&req, []*os.File{owner.directory}); err != nil {
		_ = rejectBridgeCall(&req, StatusInvalidArgument)
		t.Fatal("operation paths refused")
	}
	if req.operation == OpSnapshot {
		if err := pinMetadataWorkspace(&req, workspace); err != nil {
			_ = rejectBridgeCall(&req, StatusInvalidArgument)
			t.Fatal("metadata workspace refused")
		}
	}
	id, status := startBridgeCall(req)
	if status != StatusOk {
		t.Fatal("operation start refused", status)
	}
	handlesMu.Lock()
	handle := handles[id]
	handlesMu.Unlock()
	t.Cleanup(func() {
		_ = cancelCall(id)
		<-handle.doneCh
		_ = freeCall(id)
	})
	return id, handle
}

func s3TestWaitOperation(t *testing.T, id uint64, handle *callHandle) bridgeResult {
	t.Helper()
	select {
	case <-handle.doneCh:
	case <-time.After(45 * time.Second):
		t.Fatal("owned operation did not return")
	}
	handle.mu.Lock()
	result := handle.result
	if len(result.metadata) > 0 {
		result.metadata = append([]byte(nil), result.metadata...)
	}
	handle.mu.Unlock()
	var document struct {
		Kind string `json:"kind"`
	}
	if json.Unmarshal(result.metadata, &document) == nil && document.Kind == "recovery-add-recorded" {
		public, status := sourceCMetadata(id)
		if status != StatusOk || !bytes.Equal(public, result.metadata) {
			t.Fatal("recorded recovery candidate unavailable through public C accessor", status)
		}
	}
	if freeCall(id) != StatusOk {
		t.Fatal("completed operation did not release handle")
	}
	return result
}

func TestS3OperationsEncryptedRoundTripThroughOwnedWorker(t *testing.T) {
	cfg, state, workspace := s3TestConfig(), s3OwnerTestDirectory(t), s3OwnerTestDirectory(t)
	peer := &s3MemoryPeer{cfg: cfg, objects: make(map[string][]byte)}
	base, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	source := filepath.Join(base, "source")
	if os.Mkdir(source, 0700) != nil {
		t.Fatal("source fixture failed")
	}
	firstBytes := []byte("synthetic owner journal original for remote round trip")
	changedBytes := []byte("synthetic owner journal changed for remote round trip")
	if os.WriteFile(filepath.Join(source, "journal.txt"), firstBytes, 0640) != nil || os.Symlink("journal.txt", filepath.Join(source, "journal-link")) != nil {
		t.Fatal("source fixture failed")
	}
	daily, recovery := []byte("remote-operation-daily-fixture"), []byte("remote-operation-distinct-recovery-fixture")
	call := func(operation int32, password []byte, restore, snapshot string) bridgeResult {
		req := bridgeRequest{operation: operation, password: append([]byte(nil), password...), restorePath: restore, snapshotID: snapshot}
		if operation == OpInit || operation == OpAddRecoveryKey {
			req.recoveryPassword = append([]byte(nil), recovery...)
		}
		if operation == OpSnapshot || operation == OpRestore {
			req.sourcePath = source
		}
		id, handle := s3TestStartOperation(t, cfg, state, workspace, peer, req)
		return s3TestWaitOperation(t, id, handle)
	}
	initResult := call(OpInit, daily, "", "")
	if initResult.status != StatusOk || !validS3ID(initResult.keyID) {
		t.Fatal("actual encrypted init failed", initResult.status, initResult.diagnostic)
	}
	added := call(OpAddRecoveryKey, daily, "", "")
	if added.status != StatusOk || !validS3ID(added.keyID) || added.keyID == initResult.keyID {
		t.Fatal("independent recovery creation failed", added.status, added.diagnostic)
	}
	for _, credential := range []struct {
		password []byte
		id       string
	}{{daily, initResult.keyID}, {recovery, added.keyID}} {
		opened := call(OpOpen, credential.password, "", "")
		if opened.status != StatusOk || opened.keyID != credential.id {
			t.Fatal("independent key open failed", opened.status, opened.diagnostic)
		}
	}
	wrong := call(OpOpen, []byte("wrong-password"), "", "")
	if wrong.status != StatusFailed || wrong.diagnostic != DiagAuthFailed {
		t.Fatal("wrong password did not refuse", wrong.status, wrong.diagnostic)
	}
	peer.mu.Lock()
	before := make(map[string][]byte)
	for key, value := range peer.objects {
		before[key] = append([]byte(nil), value...)
	}
	peer.mu.Unlock()
	existing := call(OpInit, daily, "", "")
	if existing.status == StatusOk {
		t.Fatal("existing repository initialized again")
	}
	peer.mu.Lock()
	unchanged := len(before) == len(peer.objects)
	for key, value := range before {
		unchanged = unchanged && bytes.Equal(value, peer.objects[key])
	}
	peer.mu.Unlock()
	if !unchanged {
		t.Fatal("existing init changed remote bytes")
	}
	first := call(OpSnapshot, daily, "", "")
	if first.status != StatusOk || !validS3ID(first.snapshotID) {
		t.Fatal("original snapshot failed", first.status, first.diagnostic)
	}
	// Exercise incremental growth beyond the former eight-index census through
	// the real owned Snapshot worker, retaining encrypted content addressing.
	indexBackend := s3TestBackend(t, cfg, peer)
	indexRepo, err := repository.New(guardedBackend{indexBackend}, repository.Options{})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := safeScanKey(context.Background(), indexRepo, string(daily)); err != nil {
		t.Fatal(err)
	}
	encoder, err := zstd.NewWriter(nil, zstd.WithEncoderConcurrency(1))
	if err != nil {
		t.Fatal(err)
	}
	defer encoder.Close()
	addIndex := func(plain []byte) restic.ID {
		t.Helper()
		encoded := encoder.EncodeAll(plain, []byte{2})
		nonce := crypto.NewRandomNonce()
		ciphertext := indexRepo.Key().Seal(append([]byte(nil), nonce...), nonce, encoded, nil)
		id := restic.Hash(ciphertext)
		if err := indexBackend.Save(context.Background(), backend.Handle{Type: backend.IndexFile, Name: id.String()}, backend.NewByteReader(ciphertext, nil)); err != nil {
			t.Fatal("authenticated index fixture", err)
		}
		return id
	}
	for i := 0; i < 8; i++ {
		// A fresh nonce gives even empty indexes unique content-addressed IDs.
		addIndex([]byte(`{"packs":[]}`))
	}
	indexCount, indexBytes := 0, int64(0)
	if err := indexRepo.List(context.Background(), restic.IndexFile, func(_ restic.ID, size int64) error {
		indexCount++
		indexBytes += size
		return nil
	}); err != nil || indexCount != 9 {
		t.Fatalf("nine-index fixture: count=%d bytes=%d error=%v", indexCount, indexBytes, err)
	}
	t.Logf("owned Snapshot input: %d authenticated indexes, %d stored bytes", indexCount, indexBytes)
	if os.WriteFile(filepath.Join(source, "journal.txt"), changedBytes, 0640) != nil || os.WriteFile(filepath.Join(source, "second.txt"), []byte("second owned fixture"), 0600) != nil {
		t.Fatal("fixture update failed")
	}
	second := call(OpSnapshot, daily, "", "")
	if second.status != StatusOk || !validS3ID(second.snapshotID) || second.snapshotID == first.snapshotID {
		t.Fatal("changed snapshot failed", second.status, second.diagnostic)
	}
	listed := call(OpListSnapshots, recovery, "", "")
	if listed.status != StatusOk || listed.snapshotCount != 2 {
		t.Fatal("actual snapshot list failed", listed.status, listed.diagnostic)
	}
	for _, snapshot := range []struct {
		id, name string
		data     []byte
		extra    bool
	}{{first.snapshotID, "restore-original", firstBytes, false}, {second.snapshotID, "restore-changed", changedBytes, true}} {
		destination := filepath.Join(base, snapshot.name)
		restored := call(OpRestore, recovery, destination, snapshot.id)
		if restored.status != StatusOk {
			t.Fatal("separate recovery restore failed", restored.status, restored.diagnostic)
		}
		got, err := os.ReadFile(filepath.Join(destination, "source/journal.txt"))
		if err != nil || !bytes.Equal(got, snapshot.data) {
			t.Fatal("restored bytes changed")
		}
		link, err := os.Readlink(filepath.Join(destination, "source/journal-link"))
		if err != nil || link != "journal.txt" {
			t.Fatal("restored link changed")
		}
		_, extraErr := os.Stat(filepath.Join(destination, "source/second.txt"))
		if (extraErr == nil) != snapshot.extra {
			t.Fatal("snapshot versions were mixed")
		}
		refusedOwner, err := newS3Owner(state, cfg)
		if err != nil {
			t.Fatal("refusal owner setup failed")
		}
		refusedReq := bridgeRequest{operation: OpRestore, password: append([]byte(nil), recovery...), sourcePath: source, restorePath: destination, snapshotID: snapshot.id, remote: &s3RemoteRequest{cfg: cfg, owner: refusedOwner}}
		pinErr := pinDataPaths(&refusedReq, []*os.File{refusedOwner.directory})
		refusedStatus := rejectBridgeCall(&refusedReq, StatusInvalidArgument)
		if pinErr == nil || refusedStatus != StatusInvalidArgument {
			t.Fatal("existing destination accepted")
		}
	}
	if check := call(OpCheck, recovery, "", ""); check.status != StatusOk {
		t.Fatal("full read-data integrity failed", check.status, check.diagnostic)
	}
	actual, err := os.ReadFile(filepath.Join(source, "journal.txt"))
	if err != nil || !bytes.Equal(actual, changedBytes) {
		t.Fatal("restore changed source")
	}
	peer.mu.Lock()
	for key, value := range peer.objects {
		if bytes.Contains(value, firstBytes) || bytes.Contains(value, changedBytes) || strings.HasPrefix(key, cfg.prefix+"/locks/") {
			peer.mu.Unlock()
			t.Fatal("remote plaintext or uncleared lock remained")
		}
	}
	peer.mu.Unlock()
	owner := s3OwnerTestOpen(t, state, cfg)
	if owner.record.Active != nil || owner.record.Complete == nil || len(owner.record.Passwords) < 2 {
		t.Fatal("terminal recovery custody missing")
	}
	s3OwnerTestClose(t, owner)

	// Small encrypted objects may expand near the per-object decode limit.
	// Nine individually admissible frames must exceed the shared 64MiB work
	// budget, and the real worker must refuse rather than use a partial index.
	padded := append([]byte(`{"packs":[]}`), bytes.Repeat([]byte(" "), maxIndexBytes-1024)...)
	var paddedID restic.ID
	for i := 0; i < 9; i++ {
		paddedID = addIndex(padded)
	}
	probeRepo, err := repository.New(guardedBackend{indexBackend}, repository.Options{})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := safeScanKey(context.Background(), probeRepo, string(daily)); err != nil {
		t.Fatal(err)
	}
	// Repeated reads are charged too, not just distinct IDs or a single list.
	for i := 0; i < 9; i++ {
		_, err := probeRepo.LoadUnpacked(context.Background(), restic.IndexFile, paddedID)
		if i < 8 && err != nil || i == 8 && !errors.Is(err, repository.ErrMobileIndexBudgetExceeded) {
			t.Fatalf("repository lifetime decoded budget at read %d: %v", i+1, err)
		}
	}
	if _, err := probeRepo.LoadUnpacked(context.Background(), restic.IndexFile, restic.ID{}); !errors.Is(err, repository.ErrMobileIndexBudgetExceeded) {
		t.Fatalf("exhausted repository tried another metadata read: %v", err)
	}
	refused := call(OpSnapshot, daily, "", "")
	if refused.status != StatusCapExceeded || refused.diagnostic != DiagCapExceeded || refused.snapshotID != "" {
		t.Fatalf("partial indexes reached Snapshot: status=%d diagnostic=%q snapshot=%q", refused.status, refused.diagnostic, refused.snapshotID)
	}
	if got, err := os.ReadFile(filepath.Join(source, "journal.txt")); err != nil || !bytes.Equal(got, changedBytes) {
		t.Fatal("cap refusal changed the original")
	}
}
