// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

/*
#include <stdint.h>

#ifndef SOLSTONE_RESTIC_OPERATION_OPTIONS_V1
#define SOLSTONE_RESTIC_OPERATION_OPTIONS_V1
typedef struct {
    uint32_t version;
    uint32_t check_read_mode;
    uint64_t retry_lock_ms;
    uint32_t group;
    uint32_t groups;
    uint32_t reserved1;
    uint32_t reserved2;
} restic_operation_options_v1;
#endif


#ifndef SOLSTONE_RESTIC_LEGACY_TYPES
#define SOLSTONE_RESTIC_LEGACY_TYPES
typedef struct {
    int32_t operation;
    const char *password;
    uint32_t password_len;
    const char *recovery_password;
    uint32_t recovery_password_len;
    const char *repo_path;
    uint32_t repo_path_len;
    const char *source_path;
    uint32_t source_path_len;
    const char *restore_path;
    uint32_t restore_path_len;
    const char *snapshot_id;
    uint32_t snapshot_id_len;
} restic_request;

typedef struct {
    int32_t status;
    char key_id[65];
    char snapshot_id[65];
    uint32_t snapshot_count;
    char snapshot_ids[4096][65];
    char diagnostic[256];
} restic_result;
#endif

typedef struct {
    uint32_t version;
    uint32_t target_admitted;
    uint32_t target_may_have_mutated;
} restic_restore_root_outcome_v1;
*/
import "C"

import (
	"context"
	"errors"
	"golang.org/x/sys/unix"
	"sync"
	"sync/atomic"
	"unicode/utf8"
	"unsafe"
)

// Handle states.
const (
	handleRunning int32 = 0
	handleStopped int32 = 1
	handleFreed   int32 = 2
)

// Internal call handle tracking worker lifecycle and owned memory.
type callHandle struct {
	owned                *ownedCall
	id                   uint64
	ctx                  context.Context
	cancel               context.CancelFunc
	doneCh               chan struct{}
	result               bridgeResult
	passwords            [][]byte
	state                int32
	cancelRequested      bool
	publicationCommitted bool
	retainedS3Request    *bridgeRequest
	rootedRestore        *rootedRestore
	mu                   sync.Mutex
}

var (
	handlesMu sync.Mutex
	handles   = make(map[uint64]*callHandle)
	nextID    atomic.Uint64
)

func init() {
	// IDs start at 1; 0 is permanently invalid.
	nextID.Store(0)
}

// testGate hook for deterministic unit and integration test synchronization.
// Nil in production.
var testGate func(stage string, ctx context.Context) error

func zeroBytes(b []byte) {
	for i := range b {
		b[i] = 0
	}
}

func copyCBytes(ptr *C.char, length C.uint32_t) []byte {
	if ptr == nil || length == 0 {
		return nil
	}
	buf := make([]byte, int(length))
	src := unsafe.Slice((*byte)(unsafe.Pointer(ptr)), int(length))
	copy(buf, src)
	return buf
}

func copyCString(dst []C.char, s string) {
	if len(dst) == 0 {
		return
	}
	for i := range dst {
		dst[i] = 0
	}
	max := len(dst) - 1
	if len(s) < max {
		max = len(s)
	}
	for i := 0; i < max; i++ {
		dst[i] = C.char(s[i])
	}
	dst[max] = 0
}

// validateRequest validates C request lengths and path invariants.
func validateRequest(req *C.restic_request) (bridgeRequest, int32) {
	return validateRequestOwned(req, nil)
}

func validateRequestOwned(req *C.restic_request, entry *ownedCall) (bridgeRequest, int32) {
	if req == nil {
		return bridgeRequest{}, StatusInvalidArgument
	}

	op := int32(req.operation)
	if !validLocalOperation(op) {
		return bridgeRequest{}, StatusInvalidArgument
	}

	shape := requestShape{
		operation: op,
		fields: [6]requestField{
			{uint32(req.password_len), req.password != nil},
			{uint32(req.recovery_password_len), req.recovery_password != nil},
			{uint32(req.repo_path_len), req.repo_path != nil},
			{uint32(req.source_path_len), req.source_path != nil},
			{uint32(req.restore_path_len), req.restore_path != nil},
			{uint32(req.snapshot_id_len), req.snapshot_id != nil},
		},
	}
	if status := validateShape(shape); status != StatusOk {
		return bridgeRequest{}, status
	}

	repoBytes := copyCBytes(req.repo_path, req.repo_path_len)
	repoPath := string(repoBytes)
	if err := sanitizePath(repoPath); err != nil {
		return bridgeRequest{}, StatusInvalidArgument
	}
	if err := checkNoSymlinks(repoPath); err != nil {
		return bridgeRequest{}, StatusInvalidArgument
	}

	// Callable paths must already exist. This lets the bridge pin each directory
	// by descriptor before the asynchronous worker starts, so later renames or
	// symlink substitutions cannot redirect repository access.
	if err := validateExistingDir(repoPath); err != nil {
		return bridgeRequest{}, StatusInvalidArgument
	}

	var sourcePath string
	if op == OpSnapshot || op == OpRestore {
		srcBytes := copyCBytes(req.source_path, req.source_path_len)
		sourcePath = string(srcBytes)
		if err := validateExistingDir(sourcePath); err != nil {
			return bridgeRequest{}, StatusInvalidArgument
		}
		if pathsOverlap(repoPath, sourcePath) {
			return bridgeRequest{}, StatusInvalidArgument
		}
	}

	var restorePath string
	if op == OpRestore {
		rstBytes := copyCBytes(req.restore_path, req.restore_path_len)
		restorePath = string(rstBytes)
		if err := validateRestoreDest(restorePath); err != nil {
			return bridgeRequest{}, StatusInvalidArgument
		}
		if pathsOverlap(repoPath, restorePath) || pathsOverlap(sourcePath, restorePath) {
			return bridgeRequest{}, StatusInvalidArgument
		}
	}

	var snapshotID string
	if op == OpRestore {
		snBytes := copyCBytes(req.snapshot_id, req.snapshot_id_len)
		snapshotID = string(snBytes)
	}

	breq := bridgeRequest{
		owned:            entry,
		operation:        op,
		password:         copyCBytes(req.password, req.password_len),
		recoveryPassword: copyCBytes(req.recovery_password, req.recovery_password_len),
		repoPath:         repoPath,
		sourcePath:       sourcePath,
		restorePath:      restorePath,
		snapshotID:       snapshotID,
	}
	entry.retain(&breq)
	if testOwnedStage != nil && entry != nil {
		testOwnedStage("copied", entry)
	}
	if err := pinRequestPaths(&breq); err != nil {
		if entry != nil {
			zeroBytes(breq.password)
			zeroBytes(breq.recoveryPassword)
			return breq, StatusInvalidArgument
		}
		closeRequestPaths(&breq)
		return bridgeRequest{}, StatusInvalidArgument
	}

	return breq, StatusOk
}

func allocateID(counter *atomic.Uint64) (uint64, int32) {
	for {
		previous := counter.Load()
		if previous == ^uint64(0) {
			return 0, StatusCapExceeded
		}
		if counter.CompareAndSwap(previous, previous+1) {
			return previous + 1, StatusOk
		}
	}
}

// rejectBridgeCall cleans admitted resources without masking retained scratch.
func rejectBridgeCall(req *bridgeRequest, status int32) int32 {
	if req.owned != nil {
		zeroBytes(req.password)
		zeroBytes(req.recoveryPassword)
		err := closeRequestPaths(req)
		req.owned.blockIfFault(req)
		if req.remote != nil {
			err = errors.Join(err, req.remote.owner.Close())
		}
		req.owned.blockIfFault(req)
		if errors.Is(err, errMetadataCleanup) {
			return StatusWorkspaceCleanupRequired
		}
		if err != nil {
			return StatusFailed
		}
		return status
	}

	err := closeRequestPaths(req)
	if req.remote != nil {
		err = errors.Join(err, req.remote.owner.Close())
	}
	err = errors.Join(err, releaseRootedRestoreBorrow(req.rootedRestore))
	zeroBytes(req.password)
	zeroBytes(req.recoveryPassword)
	if errors.Is(err, errMetadataCleanup) {
		return StatusWorkspaceCleanupRequired
	}
	if err != nil {
		return StatusFailed
	}
	return status
}

// startBridgeCall allocates a handle, initializes context, and launches the worker.
func startBridgeCall(req bridgeRequest) (uint64, int32) {
	req.owned.retain(&req)
	req.owned.blockIfFault(&req)
	id, status := allocateID(&nextID)
	if status != StatusOk {
		return 0, rejectBridgeCall(&req, status)
	}

	ctx, cancel := context.WithCancel(context.Background())
	h := &callHandle{
		owned:         req.owned,
		id:            id,
		ctx:           ctx,
		cancel:        cancel,
		doneCh:        make(chan struct{}),
		state:         handleRunning,
		passwords:     [][]byte{req.password, req.recoveryPassword},
		rootedRestore: req.rootedRestore,
	}

	if req.owned != nil {
		if testOwnedStage != nil {
			testOwnedStage("before-publication", req.owned)
		}
		ownedCalls.Lock()
		if ownedCalls.sealed {
			ownedCalls.Unlock()
			cancel()
			return 0, rejectBridgeCall(&req, StatusFailed)
		}
	}
	handlesMu.Lock()
	running := 0
	for _, active := range handles {
		active.mu.Lock()
		if active.state == handleRunning {
			running++
		}
		active.mu.Unlock()
	}
	if running >= 4 || len(handles) >= 16 {
		handlesMu.Unlock()
		if req.owned != nil {
			ownedCalls.Unlock()
		}
		cancel()
		return 0, rejectBridgeCall(&req, StatusBusy)
	}
	handles[id] = h
	if req.owned != nil {
		req.owned.published = true
		req.owned.admitted.Store(true)
	}
	handlesMu.Unlock()
	if req.owned != nil {
		ownedCalls.Unlock()
	}

	if req.owned != nil && testOwnedStage != nil {
		testOwnedStage("worker-launch", req.owned)
	}
	go runWorker(h, req)

	return id, StatusOk
}

// runWorker executes the requested operation under panic and cancellation protection.
// finishCall publishes once, after native cleanup, in the same critical section as cancellation.
func finishCall(h *callHandle, result bridgeResult, wrote bool) {
	for _, password := range h.passwords {
		zeroBytes(password)
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	if (h.cancelRequested || h.ctx.Err() != nil) && result.diagnostic != DiagWorkspaceCleanupRequired {
		result = bridgeResult{status: StatusCancelled, diagnostic: DiagCancelled}
		if wrote {
			result = bridgeResult{status: StatusCancelledUncertain, diagnostic: DiagCancelledUncertain}
		}
	}
	h.result = result
	h.state = handleStopped
	h.cancel()
	close(h.doneCh)
}

// publishRestoreForHandle is the cancellation/publication linearization point.
// Whichever takes h.mu first wins: cancellation prevents publication, while a
// completed rename prevents a later cancellation from rewriting the result.
func publishRestoreForHandle(h *callHandle, req *bridgeRequest) error {
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.cancelRequested || h.ctx.Err() != nil {
		return context.Canceled
	}
	err := promoteRestoreStage(h.ctx, req)
	if err == nil || errors.Is(err, errRestoreCommittedUncertain) {
		h.publicationCommitted = true
	}
	return err
}

func runWorker(h *callHandle, req bridgeRequest) {
	if req.remote != nil {
		runS3Worker(h, req)
		return
	}
	wrote := false
	result := bridgeResult{status: StatusInternal, diagnostic: DiagInternal}
	defer func() {
		panicValue := recover()
		cleanupErr := closeRequestPaths(&req)
		req.owned.blockIfFault(&req)
		if errors.Is(cleanupErr, errMetadataCleanup) {
			result = bridgeResult{status: StatusFailed, diagnostic: DiagWorkspaceCleanupRequired}
		} else if panicValue != nil {
			result = bridgeResult{status: StatusInternal, diagnostic: DiagInternal}
		} else if cleanupErr != nil {
			if result.status == StatusOk && req.operation == OpRestore {
				// Publication may already be visible even though the final staging
				// census or parent sync could not be confirmed.
				result = bridgeResult{status: StatusCommittedUncertain, diagnostic: DiagCommittedUncertain}
			} else if result.status != StatusCommittedUncertain {
				result = bridgeResult{status: StatusFailed, diagnostic: DiagIoFailed}
			}
		}
		finishCall(h, result, wrote)
	}()
	if req.metadataRoot != nil {
		if err := cleanupMetadataWorkspace(&req); err != nil {
			result = bridgeResult{status: StatusFailed, diagnostic: DiagWorkspaceCleanupRequired}
			return
		}
	}
	if testGate != nil {
		if err := testGate("before-restic", h.ctx); err != nil {
			result = bridgeResult{status: StatusCancelled, diagnostic: DiagCancelled}
			return
		}
	}
	if h.ctx.Err() != nil {
		result = bridgeResult{status: StatusCancelled, diagnostic: DiagCancelled}
		return
	}
	markWrite := func() { wrote = true }
	switch req.operation {
	case OpInit:
		result, _ = executeInit(h.ctx, req, markWrite)
	case OpOpen:
		result, _ = executeOpen(h.ctx, req)
	case OpAddRecoveryKey:
		result, _ = executeAddRecoveryKey(h.ctx, req, markWrite)
	case OpSnapshot:
		result, _ = executeSnapshot(h.ctx, req, markWrite)
	case OpListSnapshots:
		result, _ = executeListSnapshots(h.ctx, req)
	case OpRestore:
		if req.rootedRestore != nil {
			result, _ = executeRootedRestore(h.ctx, &req, markWrite)
		} else {
			result, _ = executeRestoreWithPublisher(h.ctx, &req, markWrite, func(_ context.Context, req *bridgeRequest) error {
				return publishRestoreForHandle(h, req)
			})
		}
	case OpCheck:
		result, _ = executeCheck(h.ctx, req)
	case OpListRecoveryKeys:
		result, _ = executeListRecoveryKeys(h.ctx, req)
	case OpRemoveRecoveryKey:
		result, _ = executeRemoveRecoveryKey(h.ctx, req, markWrite)
	case OpArchiveSources:
		result, _ = executeArchiveSources(h.ctx, req, markWrite)
	case OpListNodes:
		result, _ = executeListNodes(h.ctx, req)
	case OpReadFile:
		result, _ = executeReadFile(h.ctx, req, markWrite)
	case OpRetention:
		result, _ = executeRetention(h.ctx, req, markWrite)
	default:
		result = bridgeResult{status: StatusInvalidArgument, diagnostic: DiagInvalidArgument}
	}
}

// parseCOperationOptions takes an immutable owned copy before a worker exists.
// Version 2 only adds remote append-only, so local entries accept version 1 only.
func parseCOperationOptions(operation int32, options *C.restic_operation_options_v1) (*operationOptions, int32) {
	if options != nil && options.version != 1 {
		return nil, StatusInvalidArgument
	}
	return parseCRemoteOperationOptions(operation, options)
}

func parseCRemoteOperationOptions(operation int32, options *C.restic_operation_options_v1) (*operationOptions, int32) {
	if options == nil {
		return nil, StatusInvalidArgument
	}
	return parseOperationOptions(operation, uint32(options.version), uint32(options.check_read_mode), uint64(options.retry_lock_ms), uint32(options.group), uint32(options.groups), uint32(options.reserved1), uint32(options.reserved2))
}

//export restic_bridge_operation_v1_start
func restic_bridge_operation_v1_start(request *C.restic_request, options *C.restic_operation_options_v1, workspace *C.char, workspaceLen C.uint32_t, policy *C.char, policyLen C.uint32_t, outID *C.uint64_t) (returned C.int32_t) {
	if outID == nil {
		return C.int32_t(StatusInvalidArgument)
	}
	*outID = 0
	entry, status := reserveOwnedCall()
	if status != StatusOk {
		return C.int32_t(status)
	}
	defer func() { returned = C.int32_t(entry.finishAdmission(int32(returned))) }()
	if request == nil {
		return C.int32_t(StatusInvalidArgument)
	}
	ownedOptions, status := parseCOperationOptions(int32(request.operation), options)
	if status != StatusOk {
		return C.int32_t(status)
	}
	var retention *retentionPolicy
	if request.operation == C.int32_t(OpSnapshot) {
		selection, status := parseCSnapshotSelection(policy, policyLen)
		if status != StatusOk {
			return C.int32_t(status)
		}
		return startWithWorkspaceSourceOwned(request, workspace, workspaceLen, selection, ownedOptions, nil, entry, outID)
	} else if request.operation == C.int32_t(OpRetention) {
		if workspaceLen != 0 || request.recovery_password_len != 0 || request.source_path_len != 0 || request.restore_path_len != 0 || request.snapshot_id_len != 0 {
			return C.int32_t(StatusInvalidArgument)
		}
		retention, status = parseCRetentionPolicy(policy, policyLen)
		if status != StatusOk {
			return C.int32_t(status)
		}
	} else if workspaceLen != 0 || policyLen != 0 {
		return C.int32_t(StatusInvalidArgument)
	}
	req, status := validateRequestOwned(request, entry)
	if status != StatusOk {
		return C.int32_t(status)
	}
	req.executionOptions = ownedOptions
	req.retentionPolicy = retention
	id, status := startBridgeCall(req)
	if status == StatusOk {
		*outID = C.uint64_t(id)
	}
	return C.int32_t(status)
}

//export restic_bridge_remove_recovery_key_v1_start
func restic_bridge_remove_recovery_key_v1_start(request *C.restic_request, options *C.restic_operation_options_v1, keyID *C.char, keyIDLen C.uint32_t, maintenance C.uint32_t, outID *C.uint64_t) (returned C.int32_t) {
	if outID == nil {
		return C.int32_t(StatusInvalidArgument)
	}
	*outID = 0
	if maintenance != 1 {
		return C.int32_t(StatusInvalidArgument)
	}
	entry, status := reserveOwnedCall()
	if status != StatusOk {
		return C.int32_t(status)
	}
	defer func() { returned = C.int32_t(entry.finishAdmission(int32(returned))) }()
	if request == nil || request.operation != C.int32_t(OpRemoveRecoveryKey) || keyID == nil || keyIDLen != 64 {
		return C.int32_t(StatusInvalidArgument)
	}
	keyBytes := copyCBytes(keyID, keyIDLen)
	keyStr := string(keyBytes)
	zeroBytes(keyBytes)
	if !validS3ID(keyStr) {
		return C.int32_t(StatusInvalidArgument)
	}
	ownedOptions, status := parseCOperationOptions(int32(request.operation), options)
	if status != StatusOk {
		return C.int32_t(status)
	}
	req, status := validateRequestOwned(request, entry)
	if status != StatusOk {
		return C.int32_t(status)
	}
	req.executionOptions = ownedOptions
	req.removeKeyID = keyStr
	req.maintenanceRotation = true
	id, status := startBridgeCall(req)
	if status == StatusOk {
		*outID = C.uint64_t(id)
	}
	return C.int32_t(status)
}

//export restic_bridge_operation_directories_v1_start
func restic_bridge_operation_directories_v1_start(request *C.restic_request, options *C.restic_operation_options_v1, sourceFD C.int32_t, workspaceFD C.int32_t, workspace *C.char, workspaceLen C.uint32_t, policy *C.char, policyLen C.uint32_t, outID *C.uint64_t) (returned C.int32_t) {
	if outID == nil {
		return C.int32_t(StatusInvalidArgument)
	}
	*outID = 0
	entry, status := reserveOwnedCall()
	if status != StatusOk {
		return C.int32_t(status)
	}
	defer func() { returned = C.int32_t(entry.finishAdmission(int32(returned))) }()
	if request == nil || !normalDirectoryOperation(int32(request.operation)) || request.restore_path_len != 0 || request.snapshot_id_len != 0 {
		return C.int32_t(StatusInvalidArgument)
	}
	ownedOptions, status := parseCOperationOptions(int32(request.operation), options)
	if status != StatusOk {
		return C.int32_t(status)
	}
	var retention *retentionPolicy
	if request.operation == C.int32_t(OpSnapshot) {
		selection, status := parseCSnapshotSelection(policy, policyLen)
		if status != StatusOk {
			return C.int32_t(status)
		}
		source, status := readBorrowedSourceIdentity(int(sourceFD))
		if status != StatusOk {
			return C.int32_t(status)
		}
		metadata, status := readBorrowedDirectoryIdentity(int(workspaceFD))
		if status != StatusOk {
			return C.int32_t(status)
		}
		return startWithWorkspaceDirectoriesOwned(request, workspace, workspaceLen, selection, ownedOptions, source, metadata, entry, outID)
	} else if request.operation == C.int32_t(OpRetention) {
		if sourceFD != -1 || workspaceFD != -1 || request.source_path_len != 0 || workspaceLen != 0 || request.recovery_password_len != 0 {
			return C.int32_t(StatusInvalidArgument)
		}
		retention, status = parseCRetentionPolicy(policy, policyLen)
		if status != StatusOk {
			return C.int32_t(status)
		}
	} else if sourceFD != -1 || workspaceFD != -1 || request.source_path_len != 0 || workspaceLen != 0 || policyLen != 0 {
		return C.int32_t(StatusInvalidArgument)
	}
	req, status := validateRequestOwned(request, entry)
	if status != StatusOk {
		return C.int32_t(status)
	}
	req.executionOptions = ownedOptions
	req.retentionPolicy = retention
	id, status := startBridgeCall(req)
	if status == StatusOk {
		*outID = C.uint64_t(id)
	}
	return C.int32_t(status)
}

//export restic_bridge_start
func restic_bridge_start(req *C.restic_request, outID *C.uint64_t) C.int32_t {
	if outID == nil {
		return C.int32_t(StatusInvalidArgument)
	}
	*outID = 0
	if req == nil {
		return C.int32_t(StatusInvalidArgument)
	}

	breq, status := validateRequest(req)
	if status != StatusOk {
		return C.int32_t(status)
	}

	id, status := startBridgeCall(breq)
	if status == StatusOk {
		*outID = C.uint64_t(id)
	}
	return C.int32_t(status)
}

// New entrypoint: existing root, exact authenticated subtree, no old source.
// The target descriptor is borrowed only for synchronous duplication.
//
//export restic_bridge_restore_root_v1_start
func restic_bridge_restore_root_v1_start(version C.uint32_t, request *C.restic_request, targetFD C.int32_t, selector *C.char, selectorLen C.uint32_t, outID *C.uint64_t) C.int32_t {
	return startLocalRootedRestore(version, request, selector, selectorLen, outID, func(value string) (*rootedRestore, int32) {
		return newRootedRestore(int(targetFD), value)
	})
}

//export restic_bridge_restore_admitted_v1_start
func restic_bridge_restore_admitted_v1_start(version C.uint32_t, request *C.restic_request, targetID C.uint64_t, selector *C.char, selectorLen C.uint32_t, outID *C.uint64_t) C.int32_t {
	return startLocalRootedRestore(version, request, selector, selectorLen, outID, func(value string) (*rootedRestore, int32) {
		return borrowRootedTarget(uint64(targetID), value)
	})
}

func startLocalRootedRestore(version C.uint32_t, request *C.restic_request, selector *C.char, selectorLen C.uint32_t, outID *C.uint64_t, admit func(string) (*rootedRestore, int32)) C.int32_t {
	if outID == nil {
		return C.int32_t(StatusInvalidArgument)
	}
	*outID = 0
	if version != 1 || request == nil || request.operation != C.int32_t(OpRestore) ||
		request.source_path_len != 0 || request.recovery_password_len != 0 {
		return C.int32_t(StatusInvalidArgument)
	}
	if request.password_len == 0 || request.password == nil || request.repo_path_len == 0 || request.repo_path == nil ||
		request.restore_path_len == 0 || request.restore_path == nil || request.snapshot_id_len != idHexLen || request.snapshot_id == nil || selectorLen == 0 || selector == nil {
		return C.int32_t(StatusInvalidArgument)
	}
	if request.password_len > maxPasswordLen || request.repo_path_len > maxPathLen || request.restore_path_len > maxPathLen || selectorLen > maxPathLen {
		return C.int32_t(StatusCapExceeded)
	}
	r, status := admit(string(copyCBytes(selector, selectorLen)))
	if status != StatusOk {
		return C.int32_t(status)
	}
	req := bridgeRequest{operation: OpRestore, password: copyCBytes(request.password, request.password_len),
		repoPath: string(copyCBytes(request.repo_path, request.repo_path_len)), restorePath: string(copyCBytes(request.restore_path, request.restore_path_len)),
		snapshotID: string(copyCBytes(request.snapshot_id, request.snapshot_id_len)), rootedRestore: r}
	if !lowerHexID(req.snapshotID) || !utf8.ValidString(req.repoPath) || !utf8.ValidString(req.restorePath) || validateExistingDir(req.repoPath) != nil || validateExistingDir(req.restorePath) != nil {
		return C.int32_t(rejectBridgeCall(&req, StatusInvalidArgument))
	}
	if err := pinRequestPaths(&req); err != nil {
		status := StatusInvalidArgument
		if errors.Is(err, errS3OwnerBusy) || errors.Is(err, unix.EWOULDBLOCK) || errors.Is(err, unix.EAGAIN) {
			status = StatusBusy
		}
		return C.int32_t(rejectBridgeCall(&req, status))
	}
	id, status := startBridgeCall(req)
	if status == StatusOk {
		*outID = C.uint64_t(id)
	}
	return C.int32_t(status)
}

//export restic_bridge_restore_root_v1_outcome
func restic_bridge_restore_root_v1_outcome(id C.uint64_t, version C.uint32_t, out *C.restic_restore_root_outcome_v1) C.int32_t {
	if out == nil {
		return C.int32_t(StatusInvalidArgument)
	}
	*out = C.restic_restore_root_outcome_v1{}
	if version != 1 {
		return C.int32_t(StatusInvalidArgument)
	}
	return C.int32_t(withRootedRestoreOutcome(uint64(id), func(admitted, mutated bool) int32 {
		out.version = 1
		if admitted {
			out.target_admitted = 1
		}
		if mutated {
			out.target_may_have_mutated = 1
		}
		return StatusOk
	}))
}

// A synchronous return of 10 means admitted scratch cleanup requires a retry.
//
//export restic_bridge_start_with_workspace
func restic_bridge_start_with_workspace(req *C.restic_request, workspace *C.char, workspaceLen C.uint32_t, outID *C.uint64_t) C.int32_t {
	return startWithWorkspace(req, workspace, workspaceLen, nil, outID)
}

func parseCSnapshotSelection(pointer *C.char, length C.uint32_t) (*snapshotSelection, int32) {
	if length > C.uint32_t(maxSnapshotSelectionBytes) {
		return nil, StatusCapExceeded
	}
	if pointer == nil || length == 0 {
		return nil, StatusInvalidArgument
	}
	return parseSnapshotSelection(copyCBytes(pointer, length))
}

func parseCRetentionPolicy(pointer *C.char, length C.uint32_t) (*retentionPolicy, int32) {
	if pointer == nil || length == 0 || length > 256 {
		return nil, StatusInvalidArgument
	}
	buf := copyCBytes(pointer, length)
	policy, status := parseRetentionPolicy(buf)
	zeroBytes(buf)
	return policy, status
}

//export restic_bridge_snapshot_v1_start
func restic_bridge_snapshot_v1_start(req *C.restic_request, workspace *C.char, workspaceLen C.uint32_t, policy *C.char, policyLen C.uint32_t, outID *C.uint64_t) C.int32_t {
	if outID == nil {
		return C.int32_t(StatusInvalidArgument)
	}
	*outID = 0
	if req == nil || req.operation != C.int32_t(OpSnapshot) {
		return C.int32_t(StatusInvalidArgument)
	}
	selection, status := parseCSnapshotSelection(policy, policyLen)
	if status != StatusOk {
		return C.int32_t(status)
	}
	return startWithWorkspace(req, workspace, workspaceLen, selection, outID)
}

func startWithWorkspace(req *C.restic_request, workspace *C.char, workspaceLen C.uint32_t, selection *snapshotSelection, outID *C.uint64_t) C.int32_t {
	return startWithWorkspaceOptions(req, workspace, workspaceLen, selection, nil, outID)
}

func startWithWorkspaceOptions(req *C.restic_request, workspace *C.char, workspaceLen C.uint32_t, selection *snapshotSelection, options *operationOptions, outID *C.uint64_t) C.int32_t {
	return startWithWorkspaceSource(req, workspace, workspaceLen, selection, options, nil, outID)
}

//export restic_bridge_snapshot_source_v1_start
func restic_bridge_snapshot_source_v1_start(req *C.restic_request, options *C.restic_operation_options_v1, sourceFD C.int32_t, workspace *C.char, workspaceLen C.uint32_t, policy *C.char, policyLen C.uint32_t, outID *C.uint64_t) (returned C.int32_t) {
	if outID == nil {
		return C.int32_t(StatusInvalidArgument)
	}
	*outID = 0
	entry, status := reserveOwnedCall()
	if status != StatusOk {
		return C.int32_t(status)
	}
	defer func() { returned = C.int32_t(entry.finishAdmission(int32(returned))) }()
	if req == nil || req.operation != C.int32_t(OpSnapshot) {
		return C.int32_t(StatusInvalidArgument)
	}
	ownedOptions, status := parseCOperationOptions(int32(req.operation), options)
	if status != StatusOk {
		return C.int32_t(status)
	}
	selection, status := parseCSnapshotSelection(policy, policyLen)
	if status != StatusOk {
		return C.int32_t(status)
	}
	expected, status := readBorrowedSourceIdentity(int(sourceFD))
	if status != StatusOk {
		return C.int32_t(status)
	}
	return startWithWorkspaceSourceOwned(req, workspace, workspaceLen, selection, ownedOptions, expected, entry, outID)
}

func startWithWorkspaceSource(req *C.restic_request, workspace *C.char, workspaceLen C.uint32_t, selection *snapshotSelection, options *operationOptions, expected *borrowedSourceIdentity, outID *C.uint64_t) C.int32_t {
	return startWithWorkspaceSourceOwned(req, workspace, workspaceLen, selection, options, expected, nil, outID)
}

func startWithWorkspaceSourceOwned(req *C.restic_request, workspace *C.char, workspaceLen C.uint32_t, selection *snapshotSelection, options *operationOptions, expected *borrowedSourceIdentity, entry *ownedCall, outID *C.uint64_t) C.int32_t {
	return startWithWorkspaceDirectoriesOwned(req, workspace, workspaceLen, selection, options, expected, nil, entry, outID)
}

func startWithWorkspaceDirectoriesOwned(req *C.restic_request, workspace *C.char, workspaceLen C.uint32_t, selection *snapshotSelection, options *operationOptions, expected *borrowedSourceIdentity, workspaceIdentity *borrowedDirectoryIdentity, entry *ownedCall, outID *C.uint64_t) C.int32_t {
	if outID == nil {
		return C.int32_t(StatusInvalidArgument)
	}
	*outID = 0
	if req == nil || req.operation != C.int32_t(OpSnapshot) || workspace == nil || workspaceLen == 0 {
		return C.int32_t(StatusInvalidArgument)
	}
	if workspaceLen > C.uint32_t(maxPathLen) {
		return C.int32_t(StatusCapExceeded)
	}
	path := string(copyCBytes(workspace, workspaceLen))
	if sanitizePath(path) != nil {
		return C.int32_t(StatusInvalidArgument)
	}
	breq, status := validateRequestOwned(req, entry)
	if status != StatusOk {
		return C.int32_t(status)
	}
	if err := pinMetadataWorkspaceExpected(&breq, path, workspaceIdentity); err != nil {
		if entry != nil {
			status := StatusInvalidArgument
			if errors.Is(err, unix.EWOULDBLOCK) || errors.Is(err, unix.EAGAIN) {
				status = StatusBusy
			}
			return C.int32_t(rejectBridgeCall(&breq, status))
		}
		_ = closeRequestPaths(&breq)
		zeroBytes(breq.password)
		zeroBytes(breq.recoveryPassword)
		if errors.Is(err, unix.EWOULDBLOCK) || errors.Is(err, unix.EAGAIN) {
			return C.int32_t(StatusBusy)
		}
		return C.int32_t(StatusInvalidArgument)
	}
	breq.snapshotSelection = selection
	breq.executionOptions = options
	if expected != nil {
		if status := verifyCapturedSource(&breq, expected); status != StatusOk {
			return C.int32_t(rejectBridgeCall(&breq, status))
		}
	}
	id, status := startBridgeCall(breq)
	if status == StatusOk {
		*outID = C.uint64_t(id)
	}
	return C.int32_t(status)
}

func cancelCall(id uint64) int32 {
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
	// Cancellation after publication cannot rewrite an already terminal result.
	if h.state == handleRunning && !h.publicationCommitted {
		h.cancelRequested = true
		h.cancel()
	}
	return StatusOk
}

//export restic_bridge_cancel
func restic_bridge_cancel(id C.uint64_t) C.int32_t { return C.int32_t(cancelCall(uint64(id))) }

//export restic_bridge_wait
func restic_bridge_wait(id C.uint64_t, out *C.restic_result) C.int32_t {
	if out == nil {
		return C.int32_t(StatusInvalidArgument)
	}

	handlesMu.Lock()
	h, ok := handles[uint64(id)]
	handlesMu.Unlock()

	if !ok || h == nil {
		return C.int32_t(StatusInvalidHandle)
	}

	h.mu.Lock()
	state := h.state
	h.mu.Unlock()

	if state == handleFreed {
		return C.int32_t(StatusInvalidHandle)
	}

	// Block until worker finishes and cleanup completes
	<-h.doneCh

	h.mu.Lock()
	res := h.result
	h.mu.Unlock()

	// Populate caller-supplied result
	out.status = C.int32_t(res.status)
	out.snapshot_count = C.uint32_t(res.snapshotCount)

	copyCString(out.key_id[:], res.keyID)
	copyCString(out.snapshot_id[:], res.snapshotID)
	copyCString(out.diagnostic[:], res.diagnostic)

	for i := range out.snapshot_ids {
		out.snapshot_ids[i][0] = 0
	}
	if res.status != StatusCapExceeded {
		for i := 0; i < int(res.snapshotCount) && i < maxSnapshotCount; i++ {
			copyCString(out.snapshot_ids[i][:], res.snapshotIDs[i])
		}
	}

	return C.int32_t(StatusOk)
}

func freeCall(id uint64) int32 {
	handlesMu.Lock()
	h, ok := handles[id]
	if !ok || h == nil {
		handlesMu.Unlock()
		return StatusInvalidHandle
	}

	h.mu.Lock()
	state := h.state
	if state == handleRunning {
		h.mu.Unlock()
		handlesMu.Unlock()
		return StatusBusy
	}
	if state == handleFreed {
		h.mu.Unlock()
		handlesMu.Unlock()
		return StatusInvalidHandle
	}
	if err := releaseRootedRestoreBorrow(h.rootedRestore); err != nil {
		h.mu.Unlock()
		handlesMu.Unlock()
		return StatusFailed
	}

	// Mark freed, zero memory, and remove from map
	h.state = handleFreed
	for _, b := range h.passwords {
		zeroBytes(b)
	}
	zeroBytes(h.result.metadata)
	h.result.metadata = nil
	delete(handles, id)
	h.mu.Unlock()
	handlesMu.Unlock()

	h.owned.retire()
	return StatusOk
}

//export restic_bridge_free
func restic_bridge_free(id C.uint64_t) C.int32_t { return C.int32_t(freeCall(uint64(id))) }

// restic_bridge_result_metadata copies optional repository-config or snapshot-
// catalog JSON into caller-owned bytes, without a trailing NUL. Existing C
// request/result layouts and operation results are unchanged. A NULL buffer
// with capacity0 queries the required size and returns CAP_EXCEEDED; callers
// must still inspect the actual terminal operation result before using metadata.
// No partial document is copied on refusal. Free remains the handle owner.
//
//export restic_bridge_result_metadata
func restic_bridge_result_metadata(id C.uint64_t, buffer *C.char, capacity C.uint32_t, required *C.uint32_t) C.int32_t {
	if required == nil {
		return C.int32_t(StatusInvalidArgument)
	}
	*required = 0
	if uint64(capacity) > maxResultMetadata || buffer == nil && capacity != 0 {
		return C.int32_t(StatusInvalidArgument)
	}
	return C.int32_t(withResultMetadata(uint64(id), func(payload []byte) int32 {
		*required = C.uint32_t(len(payload))
		if uint64(capacity) < uint64(len(payload)) {
			return StatusCapExceeded
		}
		copy(unsafe.Slice((*byte)(unsafe.Pointer(buffer)), int(capacity)), payload)
		return StatusOk
	}))
}

func main() {}
