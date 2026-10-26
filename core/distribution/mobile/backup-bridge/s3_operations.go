// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"time"

	"github.com/restic/restic/internal/backend"
	"github.com/restic/restic/internal/restic"
	"golang.org/x/sys/unix"
)

const (
	OpS3Inspect                       int32 = 8
	StatusRemotePending               int32 = 11
	DiagS3StateNew                          = "S3_STATE_NEW"
	DiagS3StateReady                        = "S3_STATE_READY"
	DiagRemotePending                       = "REMOTE_PENDING"
	DiagRemotePendingRestore                = "REMOTE_PENDING_RESTORE_COMMITTED"
	DiagRemotePendingWorkspace              = "REMOTE_PENDING_WORKSPACE_CLEANUP_REQUIRED"
	DiagStateCommitUncertain                = "STATE_COMMITTED_UNCERTAIN"
	DiagStateCommitUncertainRestore         = "STATE_COMMITTED_UNCERTAIN_RESTORE_COMMITTED"
	DiagStateCommitUncertainWorkspace       = "STATE_COMMITTED_UNCERTAIN_WORKSPACE_CLEANUP_REQUIRED"
	DiagLocalCleanupRequired                = "LOCAL_CLEANUP_REQUIRED"
	DiagRestoreCommittedLocalCleanup        = "RESTORE_COMMITTED_LOCAL_CLEANUP_REQUIRED"
)

type s3RemoteRequest struct {
	cfg   s3Config
	owner *s3Owner
	// Production C admission leaves this nil and uses the fixed linked factory.
	// Controlled test constructors can inject a private, explicitly owned peer.
	factory func(context.Context, s3Config) (*s3Backend, error)
}

// The pinned SDK returns these exact non-secret namespace refusal errors. Other
// failures do not establish repository existence; never forward provider text.
func s3InitFailureDiagnostic(err error) string {
	if errors.Is(err, errS3Denied) {
		return DiagAuthFailed
	}
	if errors.Is(err, errS3InitExisting) {
		return DiagRepoExists
	}
	switch err.Error() {
	case "repository master key and config already initialized":
		return DiagRepoExists
	default:
		return DiagIoFailed
	}
}

func (request *s3RemoteRequest) backend(ctx context.Context) (*s3Backend, error) {
	if request.factory != nil {
		return request.factory(ctx, request.cfg)
	}
	return newS3Backend(ctx, request.cfg)
}

func operationBackend(ctx context.Context, req bridgeRequest, create bool) (backend.Backend, error) {
	if req.remote == nil {
		if create {
			return createRootBackend(req.repoRoot)
		}
		return newRootBackend(req.repoRoot)
	}
	be, err := req.remote.backend(ctx)
	if err != nil {
		return nil, err
	}
	return req.remote.owner.attachBackend(be)
}

func cleanupS3Locks(request *s3RemoteRequest) error {
	owner := request.owner
	ids := owner.unacknowledgedLocks()
	if len(ids) == 0 {
		return nil
	}
	owner.mu.Lock()
	if owner.closed || !owner.primaryDrained || owner.cleanupActive || owner.liveID == "" {
		owner.mu.Unlock()
		return errS3OwnerInvalid
	}
	owner.cleanupActive = true
	owner.mu.Unlock()
	defer func() { owner.mu.Lock(); owner.cleanupActive = false; owner.mu.Unlock() }()
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	base, err := request.backend(ctx)
	if err != nil {
		return err
	}
	be := &s3OperationBackend{s3Backend: base, owner: owner, cleanup: true}
	defer be.Close()
	var result error
	for _, id := range ids {
		if ctx.Err() != nil {
			return errors.Join(result, ctx.Err())
		}
		result = errors.Join(result, be.Remove(ctx, backend.Handle{Type: backend.LockFile, Name: id}))
	}
	return result
}

// removeInheritedS3Locks deletes the locks this attempt adopted from an
// interrupted one before any repository work, so an adopted lock of the
// conflicting kind can never stall the operation that would remove it.
func removeInheritedS3Locks(request *s3RemoteRequest) (err error) {
	defer func() {
		if recover() != nil {
			err = errS3OwnerIO
		}
	}()
	owner := request.owner
	ids := owner.unacknowledgedLocks()
	if len(ids) == 0 {
		return nil
	}
	owner.mu.Lock()
	if owner.closed || owner.cleanupActive || owner.liveID == "" {
		owner.mu.Unlock()
		return errS3OwnerInvalid
	}
	owner.cleanupActive = true
	owner.mu.Unlock()
	defer func() { owner.mu.Lock(); owner.cleanupActive = false; owner.mu.Unlock() }()
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	base, err := request.backend(ctx)
	if err != nil {
		return err
	}
	be := &s3OperationBackend{s3Backend: base, owner: owner, cleanup: true}
	defer be.Close()
	var result error
	for _, id := range ids {
		if ctx.Err() != nil {
			return errors.Join(result, ctx.Err())
		}
		result = errors.Join(result, be.Remove(ctx, backend.Handle{Type: backend.LockFile, Name: id}))
	}
	return result
}

func beginS3Owner(owner *s3Owner, req bridgeRequest) (err error) {
	defer func() {
		if recover() != nil {
			owner.mu.Lock()
			owner.stateFailed = true
			owner.mu.Unlock()
			err = errS3OwnerIO
		}
	}()
	if req.operation == OpRemoveRecoveryKey {
		owner.mu.Lock()
		prior := owner.record.Active
		drifted := prior != nil && prior.Operation == OpRemoveRecoveryKey && prior.MayHaveWritten && prior.KeyID != "" && prior.KeyID != req.removeKeyID
		owner.mu.Unlock()
		if drifted {
			return errS3OwnerPending
		}
	}
	if req.operation == OpTeardown {
		return owner.beginTeardown(req.remote.cfg, req.teardownAdmission, req.teardownIDs, req.password)
	}
	return owner.begin(req.operation, req.rotationCustody, req.remote.cfg, req.password, req.recoveryPassword)
}

func safelyCleanupS3Locks(request *s3RemoteRequest) (err error) {
	defer func() {
		if recover() != nil {
			err = errS3OwnerIO
		}
	}()
	return cleanupS3Locks(request)
}

func executeS3Operation(h *callHandle, req *bridgeRequest, wrote *bool) (result bridgeResult) {
	defer func() {
		if recover() != nil {
			result.status, result.diagnostic = StatusInternal, DiagInternal
		}
	}()
	markWrite := func() { *wrote = true }
	if req.metadataRoot != nil {
		if cleanupMetadataWorkspace(req) != nil {
			return bridgeResult{status: StatusWorkspaceCleanupRequired, diagnostic: DiagWorkspaceCleanupRequired}
		}
	}
	if testGate != nil {
		if err := testGate("before-restic", h.ctx); err != nil {
			return bridgeResult{status: StatusCancelled, diagnostic: DiagCancelled}
		}
	}
	if h.ctx.Err() != nil {
		return bridgeResult{status: StatusCancelled, diagnostic: DiagCancelled}
	}
	switch req.operation {
	case OpInit:
		result, _ = executeInit(h.ctx, *req, markWrite)
	case OpOpen:
		result, _ = executeOpen(h.ctx, *req)
	case OpAddRecoveryKey:
		result, _ = executeAddRecoveryKey(h.ctx, *req, markWrite)
	case OpSnapshot:
		result, _ = executeSnapshot(h.ctx, *req, markWrite)
	case OpListSnapshots:
		result, _ = executeListSnapshots(h.ctx, *req)
	case OpRestore:
		if req.rootedRestore != nil {
			result, _ = executeRootedRestore(h.ctx, req, markWrite)
		} else {
			result, _ = executeRestoreWithPublisher(h.ctx, req, markWrite, func(_ context.Context, req *bridgeRequest) error { return publishRestoreForHandle(h, req) })
		}
	case OpCheck:
		result, _ = executeCheck(h.ctx, *req)
	case OpListRecoveryKeys:
		result, _ = executeListRecoveryKeys(h.ctx, *req)
	case OpRemoveRecoveryKey:
		result, _ = executeRemoveRecoveryKey(h.ctx, *req, markWrite)
	case OpArchiveSources:
		result, _ = executeArchiveSources(h.ctx, *req, markWrite)
	case OpListNodes:
		result, _ = executeListNodes(h.ctx, *req)
	case OpReadFile:
		result, _ = executeReadFile(h.ctx, *req, markWrite)
	case OpTeardown:
		var admitted []restic.ID
		for _, value := range req.teardownIDs {
			id, err := restic.ParseID(value)
			if err != nil {
				return bridgeResult{status: StatusInvalidArgument, diagnostic: DiagInvalidArgument}
			}
			admitted = append(admitted, id)
		}
		var changed bool
		result, changed = executeTeardown(withTeardownScope(h.ctx), h, req, admitted)
		*wrote = *wrote || changed
	case OpRetention:
		result, _ = executeRetention(h.ctx, *req, markWrite)
	default:
		result = bridgeResult{status: StatusInvalidArgument, diagnostic: DiagInvalidArgument}
	}
	return result
}

func completeS3Owner(owner *s3Owner) (published bool, err error) {
	defer func() {
		if recover() != nil {
			owner.mu.Lock()
			published = owner.record.Complete != nil
			owner.mu.Unlock()
			err = errS3OwnerIO
		}
	}()
	return owner.complete()
}

// This formatter preserves known IDs and observed local publication while
// retaining explicit remote/state/workspace requirements through cancellation.
func finishS3Call(h *callHandle, result bridgeResult, wrote, pending, stateUncertain, workspace, published, localCleanup bool) {
	for _, password := range h.passwords {
		zeroBytes(password)
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	switch {
	case pending:
		result.status, result.diagnostic = StatusRemotePending, DiagRemotePending
		if published {
			result.diagnostic = DiagRemotePendingRestore
		}
		if workspace {
			result.diagnostic = DiagRemotePendingWorkspace
		} else if localCleanup {
			result.diagnostic += "_LOCAL_CLEANUP_REQUIRED"
		}
	case stateUncertain:
		result.status, result.diagnostic = StatusCommittedUncertain, DiagStateCommitUncertain
		if published {
			result.diagnostic = DiagStateCommitUncertainRestore
		}
		if workspace {
			result.diagnostic = DiagStateCommitUncertainWorkspace
		} else if localCleanup {
			result.diagnostic += "_LOCAL_CLEANUP_REQUIRED"
		}
	case workspace:
		result.status, result.diagnostic = StatusWorkspaceCleanupRequired, DiagWorkspaceCleanupRequired
	case localCleanup:
		result.status, result.diagnostic = StatusFailed, DiagLocalCleanupRequired
		if published {
			result.status, result.diagnostic = StatusCommittedUncertain, DiagRestoreCommittedLocalCleanup
		}
	case published && result.status != StatusOk:
		result.status, result.diagnostic = StatusCommittedUncertain, DiagCommittedUncertain
	case (h.cancelRequested || h.ctx.Err() != nil) && !published:
		result.status, result.diagnostic = StatusCancelled, DiagCancelled
		if wrote {
			result.status, result.diagnostic = StatusCancelledUncertain, DiagCancelledUncertain
		}
	}
	h.result, h.state = result, handleStopped
	h.cancel()
	close(h.doneCh)
}

// After confirmed SDK/native drain, release descriptors even if local cleanup
// panics. No further deletion is attempted; any residue stays explicit.
func closeS3RequestPaths(req *bridgeRequest) (resultErr error) {
	if req.owned != nil {
		return closeRequestPaths(req)
	}
	metadata := req.metadataRoot != nil || req.metadataFile != nil
	defer func() {
		if recover() == nil {
			return
		}
		resultErr = errS3OwnerIO
		if metadata {
			resultErr = errors.Join(resultErr, errMetadataCleanup)
		}
		for _, root := range []*os.Root{req.metadataRoot, req.repoRoot, req.sourceRoot, req.restoreRoot} {
			if root != nil {
				resultErr = errors.Join(resultErr, root.Close())
			}
		}
		if req.metadataFile != nil {
			resultErr = errors.Join(resultErr, closeRootedPrivateDirectory(req.metadataFile))
		}
		for _, file := range req.pathFiles {
			resultErr = errors.Join(resultErr, closeRootedPrivateDirectory(file))
		}
		if req.archiveTarget != nil {
			resultErr = errors.Join(resultErr, req.archiveTarget.Close())
		}
		req.metadataRoot, req.repoRoot, req.sourceRoot, req.restoreRoot = nil, nil, nil, nil
		req.archiveTarget = nil
		req.metadataFile, req.pathFiles = nil, nil
		req.pathsPinned = false
	}()
	return closeRequestPaths(req)
}

func runS3Worker(h *callHandle, req bridgeRequest) {
	req.owned.retain(&req)
	// If an unexpected failure prevents confirming drain, keep the full request
	// in its live handle. Cancellation alone cannot publish stopped or free roots.
	defer func() {
		if recover() == nil {
			return
		}
		h.cancel()
		if req.owned != nil {
			req.owned.markFault()
			req.owned.blockIfFault(&req)
		}
		h.mu.Lock()
		h.retainedS3Request = &req
		h.state = handleRunning
		h.mu.Unlock()
	}()
	owner := req.remote.owner
	result := bridgeResult{status: StatusInternal, diagnostic: DiagInternal}
	wrote, started, pending, stateUncertain := false, false, false, false
	if req.operation == OpS3Inspect {
		result = bridgeResult{status: StatusOk, diagnostic: DiagS3StateNew}
		if owner.exists {
			result.diagnostic = DiagS3StateReady
		}
		if owner.record.Active != nil && owner.record.Active.MayHaveWritten {
			pending = true
		}
	} else if err := beginS3Owner(owner, req); err != nil {
		if errors.Is(err, errS3RecoveryHold) {
			pending = true
			keyID, holdPw := owner.recoveryHoldInfo()
			if len(holdPw) > 0 {
				defer zeroBytes(holdPw)
			}
			resume := struct {
				Schema           string `json:"schema"`
				Kind             string `json:"kind"`
				RecoveryPassword string `json:"recovery_password"`
				KeyID            string `json:"key_id,omitempty"`
			}{
				Schema:           metadataSchema,
				Kind:             "recovery-add-recorded",
				RecoveryPassword: string(holdPw),
			}
			if lowerHexID(keyID) {
				resume.KeyID = keyID
			}
			encoded, marshalErr := json.Marshal(resume)
			if marshalErr != nil {
				pending = false
				result = bridgeResult{status: StatusFailed, diagnostic: DiagInternal}
			} else {
				result = bridgeResult{
					status:         StatusRemotePending,
					diagnostic:     DiagRemotePending,
					metadata:       encoded,
					metadataStatus: StatusOk,
					recoveryResume: true,
				}
			}
		} else {
			result = bridgeResult{status: StatusFailed, diagnostic: DiagIoFailed}
			pending = errors.Is(err, errS3OwnerPending)
			if errors.Is(err, errResourceLimit) {
				result.status, result.diagnostic = StatusCapExceeded, DiagCapExceeded
			}
		}
	} else if err := removeInheritedS3Locks(req.remote); err != nil {
		// The adopted locks stay in custody; no repository work runs over them.
		started = true
		result = bridgeResult{status: StatusFailed, diagnostic: DiagIoFailed}
	} else {
		started = true
		result = executeS3Operation(h, &req, &wrote)
	}
	// All operation defers, including actual Unlock, have returned before here.
	req.owned.blockIfFault(&req)
	drainErr := owner.drainPrimary()
	req.owned.blockIfFault(&req)
	if started && drainErr == nil {
		_ = safelyCleanupS3Locks(req.remote)
	}
	req.owned.blockIfFault(&req)
	cleanupErr := closeS3RequestPaths(&req)
	req.owned.blockIfFault(&req)
	workspace := errors.Is(cleanupErr, errMetadataCleanup) || result.diagnostic == DiagWorkspaceCleanupRequired
	if req.operation == OpTeardown && result.status == StatusOk && result.snapshotCount != 0 {
		result.status, result.diagnostic = StatusFailed, DiagIoFailed
	}
	if started {
		if drainErr != nil {
			pending = true
		} else {
			// Persist this disposition in the same completion publication, only
			// after real prune/zero-remaining, request cleanup and primary drain.
			// Failed no-write attempts may complete, but never acquire it.
			owner.mu.Lock()
			owner.teardownReconciled = req.operation == OpTeardown && result.status == StatusOk && result.snapshotCount == 0 && cleanupErr == nil && h.ctx.Err() == nil
			owner.mu.Unlock()
			if result.status != StatusOk {
				owner.noteAttemptFailed()
			}
			visible, err := completeS3Owner(owner)
			if err != nil {
				if visible {
					stateUncertain = true
				} else {
					pending = true
				}
			}
		}
	}
	req.owned.blockIfFault(&req)
	owner.mu.Lock()
	if result.keyID == "" && (req.operation == OpInit || req.operation == OpAddRecoveryKey) {
		result.keyID = owner.knownKeyID
	}
	if result.snapshotID == "" && req.operation == OpSnapshot {
		result.snapshotID = owner.knownSnapshotID
	}
	if owner.record.Active != nil && owner.record.Active.MayHaveWritten {
		wrote = true
	}
	owner.mu.Unlock()
	if cleanupErr != nil && !workspace && result.status == StatusOk {
		result.status, result.diagnostic = StatusFailed, DiagIoFailed
	}
	if req.restorePublished && cleanupErr != nil && !pending && !stateUncertain {
		result.status, result.diagnostic = StatusCommittedUncertain, DiagCommittedUncertain
	}
	if req.operation == OpTeardown && result.status == StatusOk && !pending && !stateUncertain {
		owner.mu.Lock()
		var state unix.Stat_t
		if owner.record.Complete == nil || !owner.record.Complete.TeardownReconciled || owner.record.Complete.Operation != OpTeardown || owner.stateFile == nil || unix.Fstat(int(owner.stateFile.Fd()), &state) != nil {
			result.status, result.diagnostic = StatusFailed, DiagIoFailed
		} else {
			result.metadata, _ = json.Marshal(map[string]interface{}{
				"schema": "solstone.backup-metadata.v1", "kind": "teardown-completed",
				"completion_id": owner.record.Complete.ID, "state_device": uint64(state.Dev), "state_inode": uint64(state.Ino),
				"scope": owner.record.Complete.TeardownScope, "origin": owner.record.Complete.TeardownOrigin,
				"reconciled": owner.record.Complete.TeardownReconciled,
			})
		}
		owner.mu.Unlock()
	}
	if err := owner.Close(); err != nil {
		result.status, result.diagnostic = StatusFailed, DiagIoFailed
	}
	req.owned.blockIfFault(&req)
	finishS3Call(h, result, wrote, pending, stateUncertain, workspace, req.restorePublished, cleanupErr != nil)
}
