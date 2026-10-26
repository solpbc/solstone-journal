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

#ifndef SOLSTONE_RESTIC_S3_REQUEST
#define SOLSTONE_RESTIC_S3_REQUEST
typedef struct {
    uint32_t version;
    int32_t operation;
    const char *endpoint; uint32_t endpoint_len;
    const char *region; uint32_t region_len;
    const char *bucket; uint32_t bucket_len;
    const char *prefix; uint32_t prefix_len;
    const char *access_key; uint32_t access_key_len;
    const char *secret_key; uint32_t secret_key_len;
    const char *session_token; uint32_t session_token_len;
    const char *password; uint32_t password_len;
    const char *recovery_password; uint32_t recovery_password_len;
    const char *state_path; uint32_t state_path_len;
    const char *source_path; uint32_t source_path_len;
    const char *restore_path; uint32_t restore_path_len;
    const char *snapshot_id; uint32_t snapshot_id_len;
    const char *workspace_path; uint32_t workspace_path_len;
} restic_s3_request;
#endif
*/
import "C"

import (
	"crypto/subtle"
	"errors"
	"golang.org/x/sys/unix"
	"os"
	"unicode/utf8"
)

func validateS3Shape(version uint32, operation int32, fields [14]requestField) int32 {
	return validateS3ShapeForRoot(version, operation, fields, false)
}

func validateS3ShapeForRoot(version uint32, operation int32, fields [14]requestField, rooted bool) int32 {
	if version != 1 || operation < OpInit || (operation > OpRemoveRecoveryKey && operation != OpRetention) {
		return StatusInvalidArgument
	}
	caps := [14]uint32{maxHTTPURLBytes, 64, 63, 512, 256, 1024, 16384, maxPasswordLen, maxPasswordLen, maxPathLen, maxPathLen, maxPathLen, idHexLen, maxPathLen}
	for index, field := range fields {
		if field.length > caps[index] {
			return StatusCapExceeded
		}
		if field.length != 0 && !field.present {
			return StatusInvalidArgument
		}
	}
	for _, index := range []int{0, 1, 2, 3, 4, 5, 9} {
		if fields[index].length == 0 {
			return StatusInvalidArgument
		}
	}
	if operation != OpS3Inspect && fields[7].length == 0 {
		return StatusInvalidArgument
	}
	if operation == OpS3Inspect && fields[7].length != 0 {
		return StatusInvalidArgument
	}
	if operation == OpAddRecoveryKey && fields[8].length == 0 {
		return StatusInvalidArgument
	}
	if operation != OpInit && operation != OpAddRecoveryKey && fields[8].length != 0 {
		return StatusInvalidArgument
	}
	if operation == OpSnapshot || (operation == OpRestore && !rooted) {
		if fields[10].length == 0 {
			return StatusInvalidArgument
		}
	} else if fields[10].length != 0 {
		return StatusInvalidArgument
	}
	if operation == OpRestore {
		if fields[11].length == 0 || fields[12].length != idHexLen {
			return StatusInvalidArgument
		}
	} else if fields[11].length != 0 || fields[12].length != 0 {
		return StatusInvalidArgument
	}
	if operation == OpSnapshot {
		if fields[13].length == 0 {
			return StatusInvalidArgument
		}
	} else if fields[13].length != 0 {
		return StatusInvalidArgument
	}
	return StatusOk
}

func validateS3Request(request *C.restic_s3_request) (result bridgeRequest, status int32) {
	return validateS3RequestForRoot(request, nil)
}

func validateS3RequestForRoot(request *C.restic_s3_request, rooted *rootedRestore) (result bridgeRequest, status int32) {
	return validateS3RequestOwned(request, rooted, nil)
}

func validateS3RequestOwned(request *C.restic_s3_request, rooted *rootedRestore, entry *ownedCall) (result bridgeRequest, status int32) {
	return validateS3RequestDirectoriesOwned(request, rooted, entry, nil, nil)
}

func validateS3RequestDirectoriesOwned(request *C.restic_s3_request, rooted *rootedRestore, entry *ownedCall, stateIdentity, workspaceIdentity *borrowedDirectoryIdentity) (result bridgeRequest, status int32) {
	if request == nil {
		return bridgeRequest{}, StatusInvalidArgument
	}
	fields := [14]requestField{
		{uint32(request.endpoint_len), request.endpoint != nil}, {uint32(request.region_len), request.region != nil},
		{uint32(request.bucket_len), request.bucket != nil}, {uint32(request.prefix_len), request.prefix != nil},
		{uint32(request.access_key_len), request.access_key != nil}, {uint32(request.secret_key_len), request.secret_key != nil},
		{uint32(request.session_token_len), request.session_token != nil}, {uint32(request.password_len), request.password != nil},
		{uint32(request.recovery_password_len), request.recovery_password != nil}, {uint32(request.state_path_len), request.state_path != nil},
		{uint32(request.source_path_len), request.source_path != nil}, {uint32(request.restore_path_len), request.restore_path != nil},
		{uint32(request.snapshot_id_len), request.snapshot_id != nil}, {uint32(request.workspace_path_len), request.workspace_path != nil},
	}
	operation := int32(request.operation)
	if rooted != nil && operation != OpRestore {
		return bridgeRequest{}, StatusInvalidArgument
	}
	if status := validateS3ShapeForRoot(uint32(request.version), operation, fields, rooted != nil); status != StatusOk {
		return bridgeRequest{}, status
	}
	readString := func(pointer *C.char, length C.uint32_t) string {
		owned := copyCBytes(pointer, length)
		defer zeroBytes(owned)
		return string(owned)
	}
	cfg := s3Config{endpoint: readString(request.endpoint, request.endpoint_len), region: readString(request.region, request.region_len),
		bucket: readString(request.bucket, request.bucket_len), prefix: readString(request.prefix, request.prefix_len),
		access: readString(request.access_key, request.access_key_len), secret: readString(request.secret_key, request.secret_key_len), token: readString(request.session_token, request.session_token_len)}
	if cfg.validate() != nil {
		return bridgeRequest{}, StatusInvalidArgument
	}
	result = bridgeRequest{owned: entry, operation: operation, password: copyCBytes(request.password, request.password_len), recoveryPassword: copyCBytes(request.recovery_password, request.recovery_password_len),
		sourcePath: readString(request.source_path, request.source_path_len), restorePath: readString(request.restore_path, request.restore_path_len), snapshotID: readString(request.snapshot_id, request.snapshot_id_len), rootedRestore: rooted}
	entry.retain(&result)
	if testOwnedStage != nil && entry != nil {
		testOwnedStage("copied", entry)
	}
	defer func() {
		if status != StatusOk {
			zeroBytes(result.password)
			zeroBytes(result.recoveryPassword)
		}
	}()
	if len(result.recoveryPassword) != 0 && len(result.password) == len(result.recoveryPassword) && subtle.ConstantTimeCompare(result.password, result.recoveryPassword) == 1 {
		return result, StatusInvalidArgument
	}
	if operation == OpRestore {
		if !validS3ID(result.snapshotID) {
			return result, StatusInvalidArgument
		}
		if rooted == nil && validateRestoreDest(result.restorePath) != nil {
			return result, StatusInvalidArgument
		}
		if rooted != nil && validateExistingDir(result.restorePath) != nil {
			return result, StatusInvalidArgument
		}
	}
	if result.sourcePath != "" && validateExistingDir(result.sourcePath) != nil {
		return result, StatusInvalidArgument
	}
	statePath := readString(request.state_path, request.state_path_len)
	if rooted != nil && (!utf8.ValidString(statePath) || !utf8.ValidString(result.restorePath)) {
		return result, StatusInvalidArgument
	}
	owner, err := newS3OwnerExpected(statePath, cfg, entry, stateIdentity)
	if err != nil {
		if errors.Is(err, errS3OwnerBusy) {
			return result, StatusBusy
		}
		if errors.Is(err, errS3OwnerIO) {
			return result, StatusFailed
		}
		return result, StatusInvalidArgument
	}
	result.remote = &s3RemoteRequest{cfg: cfg, owner: owner}
	if err := pinDataPaths(&result, []*os.File{owner.directory}); err != nil {
		if rooted != nil && (errors.Is(err, errS3OwnerBusy) || errors.Is(err, unix.EWOULDBLOCK) || errors.Is(err, unix.EAGAIN)) {
			return result, rejectBridgeCall(&result, StatusBusy)
		}
		return result, rejectBridgeCall(&result, StatusInvalidArgument)
	}
	if operation == OpSnapshot {
		workspace := readString(request.workspace_path, request.workspace_path_len)
		if err := pinMetadataWorkspaceExpected(&result, workspace, workspaceIdentity); err != nil {
			if errors.Is(err, unix.EWOULDBLOCK) || errors.Is(err, unix.EAGAIN) {
				return result, rejectBridgeCall(&result, StatusBusy)
			}
			return result, rejectBridgeCall(&result, StatusInvalidArgument)
		}
	}
	return result, StatusOk
}

//export restic_bridge_s3_restore_root_v1_start
func restic_bridge_s3_restore_root_v1_start(request *C.restic_s3_request, targetFD C.int32_t, selector *C.char, selectorLen C.uint32_t, outID *C.uint64_t) C.int32_t {
	return startS3RootedRestore(request, selector, selectorLen, outID, func(value string) (*rootedRestore, int32) {
		return newRootedRestore(int(targetFD), value)
	})
}

//export restic_bridge_s3_restore_admitted_v1_start
func restic_bridge_s3_restore_admitted_v1_start(request *C.restic_s3_request, targetID C.uint64_t, selector *C.char, selectorLen C.uint32_t, outID *C.uint64_t) C.int32_t {
	return startS3RootedRestore(request, selector, selectorLen, outID, func(value string) (*rootedRestore, int32) {
		return borrowRootedTarget(uint64(targetID), value)
	})
}

func startS3RootedRestore(request *C.restic_s3_request, selector *C.char, selectorLen C.uint32_t, outID *C.uint64_t, admit func(string) (*rootedRestore, int32)) C.int32_t {
	if outID == nil {
		return C.int32_t(StatusInvalidArgument)
	}
	*outID = 0
	if request == nil || request.operation != C.int32_t(OpRestore) || request.version != 1 || selector == nil || selectorLen == 0 {
		return C.int32_t(StatusInvalidArgument)
	}
	if selectorLen > maxPathLen {
		return C.int32_t(StatusCapExceeded)
	}
	rooted, status := admit(string(copyCBytes(selector, selectorLen)))
	if status != StatusOk {
		return C.int32_t(status)
	}
	req, status := validateS3RequestForRoot(request, rooted)
	if status != StatusOk {
		_ = rooted.close()
		if err := releaseRootedRestoreBorrow(rooted); err != nil {
			return C.int32_t(StatusFailed)
		}
		return C.int32_t(status)
	}
	id, status := startBridgeCall(req)
	if status == StatusOk {
		*outID = C.uint64_t(id)
	}
	return C.int32_t(status)
}

//export restic_bridge_s3_start
func restic_bridge_s3_start(request *C.restic_s3_request, outID *C.uint64_t) C.int32_t {
	if outID == nil {
		return C.int32_t(StatusInvalidArgument)
	}
	*outID = 0
	if request == nil || request.operation == C.int32_t(OpRemoveRecoveryKey) {
		return C.int32_t(StatusInvalidArgument)
	}
	req, status := validateS3Request(request)
	if status != StatusOk {
		return C.int32_t(status)
	}
	id, status := startBridgeCall(req)
	if status == StatusOk {
		*outID = C.uint64_t(id)
	}
	return C.int32_t(status)
}

//export restic_bridge_s3_snapshot_v1_start
func restic_bridge_s3_snapshot_v1_start(request *C.restic_s3_request, policy *C.char, policyLen C.uint32_t, outID *C.uint64_t) C.int32_t {
	if outID == nil {
		return C.int32_t(StatusInvalidArgument)
	}
	*outID = 0
	if request == nil || request.operation != C.int32_t(OpSnapshot) {
		return C.int32_t(StatusInvalidArgument)
	}
	// Refuse malformed policy before owner/root admission or any worker/network
	// access; the selection cannot change repository or credential authority.
	selection, status := parseCSnapshotSelection(policy, policyLen)
	if status != StatusOk {
		return C.int32_t(status)
	}
	req, status := validateS3Request(request)
	if status != StatusOk {
		return C.int32_t(status)
	}
	req.snapshotSelection = selection
	id, status := startBridgeCall(req)
	if status == StatusOk {
		*outID = C.uint64_t(id)
	}
	return C.int32_t(status)
}

//export restic_bridge_s3_operation_v1_start
func restic_bridge_s3_operation_v1_start(request *C.restic_s3_request, options *C.restic_operation_options_v1, policy *C.char, policyLen C.uint32_t, outID *C.uint64_t) (returned C.int32_t) {
	if outID == nil {
		return C.int32_t(StatusInvalidArgument)
	}
	*outID = 0
	if request == nil || request.operation == C.int32_t(OpRemoveRecoveryKey) {
		return C.int32_t(StatusInvalidArgument)
	}
	entry, status := reserveOwnedCall()
	if status != StatusOk {
		return C.int32_t(status)
	}
	defer func() { returned = C.int32_t(entry.finishAdmission(int32(returned))) }()

	return startS3OperationWithSource(request, options, nil, entry, policy, policyLen, outID)
}

//export restic_bridge_s3_snapshot_source_v1_start
func restic_bridge_s3_snapshot_source_v1_start(request *C.restic_s3_request, options *C.restic_operation_options_v1, sourceFD C.int32_t, policy *C.char, policyLen C.uint32_t, outID *C.uint64_t) (returned C.int32_t) {
	if outID == nil {
		return C.int32_t(StatusInvalidArgument)
	}
	*outID = 0
	entry, status := reserveOwnedCall()
	if status != StatusOk {
		return C.int32_t(status)
	}
	defer func() { returned = C.int32_t(entry.finishAdmission(int32(returned))) }()

	if outID == nil {
		return C.int32_t(StatusInvalidArgument)
	}
	*outID = 0
	if request == nil || request.operation != C.int32_t(OpSnapshot) {
		return C.int32_t(StatusInvalidArgument)
	}
	return startS3OperationWithSource(request, options, &sourceFD, entry, policy, policyLen, outID)
}

func startS3OperationWithSource(request *C.restic_s3_request, options *C.restic_operation_options_v1, sourceFD *C.int32_t, entry *ownedCall, policy *C.char, policyLen C.uint32_t, outID *C.uint64_t) C.int32_t {
	return startS3OperationWithDirectories(request, options, sourceFD, nil, nil, entry, policy, policyLen, outID)
}

//export restic_bridge_s3_operation_directories_v1_start
func restic_bridge_s3_operation_directories_v1_start(request *C.restic_s3_request, options *C.restic_operation_options_v1, sourceFD C.int32_t, stateFD C.int32_t, workspaceFD C.int32_t, policy *C.char, policyLen C.uint32_t, outID *C.uint64_t) (returned C.int32_t) {
	if outID == nil {
		return C.int32_t(StatusInvalidArgument)
	}
	*outID = 0
	entry, status := reserveOwnedCall()
	if status != StatusOk {
		return C.int32_t(status)
	}
	defer func() { returned = C.int32_t(entry.finishAdmission(int32(returned))) }()
	if request == nil || !normalDirectoryOperation(int32(request.operation)) {
		return C.int32_t(StatusInvalidArgument)
	}
	if request.operation != C.int32_t(OpSnapshot) && request.operation != C.int32_t(OpRetention) && (sourceFD != -1 || workspaceFD != -1 || request.source_path_len != 0 || request.workspace_path_len != 0 || policyLen != 0) {
		return C.int32_t(StatusInvalidArgument)
	}
	if request.operation == C.int32_t(OpRetention) && (sourceFD != -1 || workspaceFD != -1 || request.source_path_len != 0 || request.workspace_path_len != 0) {
		return C.int32_t(StatusInvalidArgument)
	}
	state, status := readBorrowedDirectoryIdentity(int(stateFD))
	if status != StatusOk {
		return C.int32_t(status)
	}
	var metadata *borrowedDirectoryIdentity
	var source *C.int32_t
	if request.operation == C.int32_t(OpSnapshot) {
		metadata, status = readBorrowedDirectoryIdentity(int(workspaceFD))
		if status != StatusOk {
			return C.int32_t(status)
		}
		source = &sourceFD
	}
	return startS3OperationWithDirectories(request, options, source, state, metadata, entry, policy, policyLen, outID)
}

func startS3OperationWithDirectories(request *C.restic_s3_request, options *C.restic_operation_options_v1, sourceFD *C.int32_t, stateIdentity, workspaceIdentity *borrowedDirectoryIdentity, entry *ownedCall, policy *C.char, policyLen C.uint32_t, outID *C.uint64_t) C.int32_t {
	if outID == nil {
		return C.int32_t(StatusInvalidArgument)
	}
	*outID = 0
	if request == nil {
		return C.int32_t(StatusInvalidArgument)
	}
	ownedOptions, status := parseCRemoteOperationOptions(int32(request.operation), options)
	if status != StatusOk {
		return C.int32_t(status)
	}
	var selection *snapshotSelection
	var retention *retentionPolicy
	if request.operation == C.int32_t(OpSnapshot) {
		selection, status = parseCSnapshotSelection(policy, policyLen)
		if status != StatusOk {
			return C.int32_t(status)
		}
	} else if request.operation == C.int32_t(OpRetention) {
		if request.recovery_password_len != 0 || request.source_path_len != 0 || request.restore_path_len != 0 || request.snapshot_id_len != 0 {
			return C.int32_t(StatusInvalidArgument)
		}
		retention, status = parseCRetentionPolicy(policy, policyLen)
		if status != StatusOk {
			return C.int32_t(status)
		}
	} else if policyLen != 0 {
		return C.int32_t(StatusInvalidArgument)
	}
	var expected *borrowedSourceIdentity
	if sourceFD != nil {
		expected, status = readBorrowedSourceIdentity(int(*sourceFD))
		if status != StatusOk {
			return C.int32_t(status)
		}
	}
	req, status := validateS3RequestDirectoriesOwned(request, nil, entry, stateIdentity, workspaceIdentity)
	if status != StatusOk {
		return C.int32_t(status)
	}
	if expected != nil {
		if status := verifyCapturedSource(&req, expected); status != StatusOk {
			return C.int32_t(rejectBridgeCall(&req, status))
		}
	}
	req.executionOptions = ownedOptions
	req.snapshotSelection = selection
	req.retentionPolicy = retention
	if ownedOptions.appendOnly {
		req.remote.cfg.appendOnly = true
	}
	id, status := startBridgeCall(req)
	if status == StatusOk {
		*outID = C.uint64_t(id)
	}
	return C.int32_t(status)
}

//export restic_bridge_s3_remove_recovery_key_v1_start
func restic_bridge_s3_remove_recovery_key_v1_start(request *C.restic_s3_request, options *C.restic_operation_options_v1, stateFD C.int32_t, keyID *C.char, keyIDLen C.uint32_t, maintenance C.uint32_t, outID *C.uint64_t) (returned C.int32_t) {
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
	stateIdentity, status := readBorrowedDirectoryIdentity(int(stateFD))
	if status != StatusOk {
		return C.int32_t(status)
	}
	ownedOptions, status := parseCRemoteOperationOptions(int32(request.operation), options)
	if status != StatusOk {
		return C.int32_t(status)
	}
	req, status := validateS3RequestDirectoriesOwned(request, nil, entry, stateIdentity, nil)
	if status != StatusOk {
		return C.int32_t(status)
	}
	req.executionOptions = ownedOptions
	req.removeKeyID = keyStr
	req.maintenanceRotation = true
	if ownedOptions.appendOnly {
		req.remote.cfg.appendOnly = true
	}
	id, status := startBridgeCall(req)
	if status == StatusOk {
		*outID = C.uint64_t(id)
	}
	return C.int32_t(status)
}

//export restic_bridge_s3_add_recovery_key_v1_start
func restic_bridge_s3_add_recovery_key_v1_start(request *C.restic_s3_request, options *C.restic_operation_options_v1, stateFD C.int32_t, custody C.uint32_t, outID *C.uint64_t) (returned C.int32_t) {
	if outID == nil {
		return C.int32_t(StatusInvalidArgument)
	}
	*outID = 0
	if custody != 1 {
		return C.int32_t(StatusInvalidArgument)
	}
	entry, status := reserveOwnedCall()
	if status != StatusOk {
		return C.int32_t(status)
	}
	defer func() { returned = C.int32_t(entry.finishAdmission(int32(returned))) }()
	if request == nil || request.operation != C.int32_t(OpAddRecoveryKey) {
		return C.int32_t(StatusInvalidArgument)
	}
	stateIdentity, status := readBorrowedDirectoryIdentity(int(stateFD))
	if status != StatusOk {
		return C.int32_t(status)
	}
	ownedOptions, status := parseCRemoteOperationOptions(int32(request.operation), options)
	if status != StatusOk {
		return C.int32_t(status)
	}
	req, status := validateS3RequestDirectoriesOwned(request, nil, entry, stateIdentity, nil)
	if status != StatusOk {
		return C.int32_t(status)
	}
	req.executionOptions = ownedOptions
	req.rotationCustody = true
	if ownedOptions.appendOnly {
		req.remote.cfg.appendOnly = true
	}
	id, status := startBridgeCall(req)
	if status == StatusOk {
		*outID = C.uint64_t(id)
	}
	return C.int32_t(status)
}
