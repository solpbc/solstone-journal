//go:build integration

// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Public-C entrypoint exercise helpers; excluded from production file closure.
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

typedef struct {
    uint32_t version;
    uint32_t target_admitted;
    uint32_t target_may_have_mutated;
} restic_restore_root_outcome_v1;


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

#include <stdlib.h>
#include <unistd.h>
int32_t restic_bridge_snapshot_source_v1_start(restic_request*,restic_operation_options_v1*,int32_t,const char*,uint32_t,const char*,uint32_t,uint64_t*);
int32_t restic_bridge_s3_snapshot_source_v1_start(restic_s3_request*,restic_operation_options_v1*,int32_t,const char*,uint32_t,uint64_t*);
int32_t restic_bridge_operation_v1_start(restic_request*,restic_operation_options_v1*,const char*,uint32_t,const char*,uint32_t,uint64_t*);
int32_t restic_bridge_s3_start(restic_s3_request*,uint64_t*);
int32_t restic_bridge_s3_operation_v1_start(restic_s3_request*,restic_operation_options_v1*,const char*,uint32_t,uint64_t*);
int32_t restic_bridge_operation_directories_v1_start(restic_request*,restic_operation_options_v1*,int32_t,int32_t,const char*,uint32_t,const char*,uint32_t,uint64_t*);
int32_t restic_bridge_s3_operation_directories_v1_start(restic_s3_request*,restic_operation_options_v1*,int32_t,int32_t,int32_t,const char*,uint32_t,uint64_t*);
int32_t restic_bridge_cancel(uint64_t);
int32_t restic_bridge_wait(uint64_t,restic_result*);
int32_t restic_bridge_free(uint64_t);
int32_t restic_bridge_result_metadata(uint64_t,char*,uint32_t,uint32_t*);
int32_t restic_bridge_teardown_v1_start(restic_request*,restic_s3_request*,restic_operation_options_v1*,int,const char*,uint32_t,const char*,uint32_t,uint64_t*);
int32_t restic_bridge_s3_teardown_observe_v1(int,const char*,uint32_t,const char*,uint32_t,int*,char*,uint32_t,uint32_t*);
int32_t restic_bridge_s3_teardown_retire_v1(int,const char*,uint32_t,const char*,uint32_t);
int32_t restic_bridge_s3_teardown_verify_v1(int,const char*,uint32_t,const char*,uint32_t);
int32_t restic_bridge_s3_archive_sources_v1_start(restic_s3_request*,int,int,const char*,uint32_t,const char*,uint32_t,uint32_t,uint64_t*);
int32_t restic_bridge_s3_archive_sources_v2_start(restic_s3_request*,int,int,const char*,uint32_t,const char*,uint32_t,uint32_t,uint64_t*,uint32_t*);
*/
import "C"
import "unsafe"

type sourceCInput struct {
	recovery                             []byte
	repository, source, workspace, state string
	password                             []byte
	remote                               *s3Config
}

func sourceCStart(input sourceCInput, fd int, selection []byte, retryMillis uint64) (uint64, int32) {
	return sourceCVersionedStart(input, OpSnapshot, true, fd, selection, retryMillis)
}

func sourceCVersionedStart(input sourceCInput, operation int32, sourceEntry bool, fd int, selection []byte, retryMillis uint64) (uint64, int32) {
	return sourceCDispatch(input, operation, sourceEntry, fd, selection, retryMillis, nil)
}

func directoryCStart(input sourceCInput, operation int32, sourceFD, stateFD, workspaceFD int, selection []byte, retryMillis uint64) (uint64, int32) {
	return sourceCDispatch(input, operation, false, sourceFD, selection, retryMillis, &[2]int{stateFD, workspaceFD})
}

func sourceCDispatch(input sourceCInput, operation int32, sourceEntry bool, fd int, selection []byte, retryMillis uint64, directories *[2]int) (uint64, int32) {
	var allocations []unsafe.Pointer
	defer func() {
		for _, p := range allocations {
			C.free(p)
		}
	}()
	bytes := func(value []byte) (*C.char, C.uint32_t) {
		if len(value) == 0 {
			return nil, 0
		}
		p := C.CBytes(value)
		allocations = append(allocations, p)
		return (*C.char)(p), C.uint32_t(len(value))
	}
	text := func(value string) (*C.char, C.uint32_t) { return bytes([]byte(value)) }
	options := C.restic_operation_options_v1{version: 1, retry_lock_ms: C.uint64_t(retryMillis)}
	if operation == OpCheck {
		options.check_read_mode = C.uint32_t(checkReadStructure)
	}
	policy, policyLen := bytes(selection)
	id := C.uint64_t(0xfedcba9876543210)
	var status C.int32_t
	if input.remote == nil {
		request := C.restic_request{operation: C.int32_t(operation)}
		request.password, request.password_len = bytes(input.password)
		if operation == OpInit || operation == OpAddRecoveryKey {
			request.recovery_password, request.recovery_password_len = bytes(input.recovery)
		}
		request.repo_path, request.repo_path_len = text(input.repository)
		if operation == OpSnapshot {
			request.source_path, request.source_path_len = text(input.source)
		}
		var workspace *C.char
		var workspaceLen C.uint32_t
		if operation == OpSnapshot {
			workspace, workspaceLen = text(input.workspace)
		}
		if directories != nil {
			status = C.restic_bridge_operation_directories_v1_start(&request, &options, C.int32_t(fd), C.int32_t(directories[1]), workspace, workspaceLen, policy, policyLen, &id)
		} else if sourceEntry {
			status = C.restic_bridge_snapshot_source_v1_start(&request, &options, C.int32_t(fd), workspace, workspaceLen, policy, policyLen, &id)
		} else {
			status = C.restic_bridge_operation_v1_start(&request, &options, workspace, workspaceLen, policy, policyLen, &id)
		}
	} else {
		cfg := input.remote
		request := C.restic_s3_request{version: 1, operation: C.int32_t(operation)}
		request.endpoint, request.endpoint_len = text(cfg.endpoint)
		request.region, request.region_len = text(cfg.region)
		request.bucket, request.bucket_len = text(cfg.bucket)
		request.prefix, request.prefix_len = text(cfg.prefix)
		request.access_key, request.access_key_len = text(cfg.access)
		request.secret_key, request.secret_key_len = text(cfg.secret)
		request.session_token, request.session_token_len = text(cfg.token)
		request.password, request.password_len = bytes(input.password)
		if operation == OpInit || operation == OpAddRecoveryKey {
			request.recovery_password, request.recovery_password_len = bytes(input.recovery)
		}
		request.state_path, request.state_path_len = text(input.state)
		if operation == OpSnapshot {
			request.source_path, request.source_path_len = text(input.source)
		}
		if operation == OpSnapshot {
			request.workspace_path, request.workspace_path_len = text(input.workspace)
		}
		if operation == OpS3Inspect {
			request.password, request.password_len = nil, 0
		}
		if directories != nil {
			status = C.restic_bridge_s3_operation_directories_v1_start(&request, &options, C.int32_t(fd), C.int32_t(directories[0]), C.int32_t(directories[1]), policy, policyLen, &id)
		} else if sourceEntry {
			status = C.restic_bridge_s3_snapshot_source_v1_start(&request, &options, C.int32_t(fd), policy, policyLen, &id)
		} else {
			status = C.restic_bridge_s3_operation_v1_start(&request, &options, policy, policyLen, &id)
		}
	}
	return uint64(id), int32(status)
}

func sourceCCancel(id uint64) int32 {
	return int32(C.restic_bridge_cancel(C.uint64_t(id)))
}

func sourceCFree(id uint64) int32 {
	return int32(C.restic_bridge_free(C.uint64_t(id)))
}

func sourceCWaitAndFree(id uint64) (bridgeResult, int32, int32) {
	output := (*C.restic_result)(C.calloc(1, C.size_t(C.sizeof_restic_result)))
	if output == nil {
		return bridgeResult{}, StatusInternal, StatusInternal
	}
	defer C.free(unsafe.Pointer(output))
	status := C.restic_bridge_wait(C.uint64_t(id), output)
	result := bridgeResult{status: int32(output.status), snapshotCount: uint32(output.snapshot_count), snapshotID: C.GoString(&output.snapshot_id[0]), keyID: C.GoString(&output.key_id[0]), diagnostic: C.GoString(&output.diagnostic[0])}
	freed := C.restic_bridge_free(C.uint64_t(id))
	return result, int32(status), int32(freed)
}

func sourceCS3Request(input sourceCInput, operation int32, allocations *[]unsafe.Pointer) C.restic_s3_request {
	bytes := func(value []byte) (*C.char, C.uint32_t) {
		if len(value) == 0 {
			return nil, 0
		}
		p := C.CBytes(value)
		*allocations = append(*allocations, p)
		return (*C.char)(p), C.uint32_t(len(value))
	}
	text := func(value string) (*C.char, C.uint32_t) { return bytes([]byte(value)) }

	request := C.restic_s3_request{version: 1, operation: C.int32_t(operation)}
	if input.remote != nil {
		cfg := input.remote
		request.endpoint, request.endpoint_len = text(cfg.endpoint)
		request.region, request.region_len = text(cfg.region)
		request.bucket, request.bucket_len = text(cfg.bucket)
		request.prefix, request.prefix_len = text(cfg.prefix)
		request.access_key, request.access_key_len = text(cfg.access)
		request.secret_key, request.secret_key_len = text(cfg.secret)
		request.session_token, request.session_token_len = text(cfg.token)
	}
	request.password, request.password_len = bytes(input.password)
	if operation == OpInit || operation == OpAddRecoveryKey {
		request.recovery_password, request.recovery_password_len = bytes(input.recovery)
	}
	request.state_path, request.state_path_len = text(input.state)
	if operation == OpSnapshot {
		request.source_path, request.source_path_len = text(input.source)
		request.workspace_path, request.workspace_path_len = text(input.workspace)
	}
	return request
}

func sourceCRawS3Start(input sourceCInput, operation int32) (uint64, int32) {
	var allocations []unsafe.Pointer
	defer func() {
		for _, p := range allocations {
			C.free(p)
		}
	}()
	request := sourceCS3Request(input, operation, &allocations)
	id := C.uint64_t(0xfedcba9876543210)
	status := C.restic_bridge_s3_start(&request, &id)
	return uint64(id), int32(status)
}

func sourceCArchiveStart(input sourceCInput, legacy bool, sourceFD, stateFD int, root string, names []byte) (uint64, int32, archiveStartStage) {
	var allocations []unsafe.Pointer
	defer func() {
		for _, p := range allocations {
			C.free(p)
		}
	}()
	request := sourceCS3Request(input, OpArchiveSources, &allocations)
	rootPtr := C.CString(root)
	defer C.free(unsafe.Pointer(rootPtr))
	namesPtr := C.CBytes(names)
	defer C.free(namesPtr)
	var id C.uint64_t
	stage := C.uint32_t(0xffffffff)
	var status C.int32_t
	if legacy {
		status = C.restic_bridge_s3_archive_sources_v1_start(&request, C.int(stateFD), C.int(sourceFD), rootPtr, C.uint32_t(len(root)), (*C.char)(namesPtr), C.uint32_t(len(names)), 0, &id)
	} else {
		status = C.restic_bridge_s3_archive_sources_v2_start(&request, C.int(stateFD), C.int(sourceFD), rootPtr, C.uint32_t(len(root)), (*C.char)(namesPtr), C.uint32_t(len(names)), 0, &id, &stage)
	}
	return uint64(id), int32(status), archiveStartStage(stage)
}

func sourceCRawS3OperationV1Start(input sourceCInput, operation int32) (uint64, int32) {
	var allocations []unsafe.Pointer
	defer func() {
		for _, p := range allocations {
			C.free(p)
		}
	}()
	request := sourceCS3Request(input, operation, &allocations)
	options := C.restic_operation_options_v1{version: 1}
	id := C.uint64_t(0xfedcba9876543210)
	status := C.restic_bridge_s3_operation_v1_start(&request, &options, nil, 0, &id)
	return uint64(id), int32(status)
}

// Read the real public C accessor before Free; direct result fields do not prove
// that Rust can retrieve a recorded recovery candidate.
func sourceCMetadata(id uint64) ([]byte, int32) {
	var required C.uint32_t
	status := C.restic_bridge_result_metadata(C.uint64_t(id), nil, 0, &required)
	if int32(status) != StatusCapExceeded || required == 0 || required > maxResultMetadata {
		return nil, int32(status)
	}
	buffer := C.malloc(C.size_t(required))
	if buffer == nil {
		return nil, StatusInternal
	}
	defer C.free(buffer)
	status = C.restic_bridge_result_metadata(C.uint64_t(id), (*C.char)(buffer), required, &required)
	if int32(status) != StatusOk || required > maxResultMetadata {
		return nil, int32(status)
	}
	return C.GoBytes(buffer, C.int(required)), int32(status)
}

func teardownCObserve(fd int, destination, pending []byte) (int, []byte, int32) {
	dest := C.CBytes(destination)
	defer C.free(dest)
	scope := C.CBytes(pending)
	defer C.free(scope)
	buffer := C.malloc(4096)
	if buffer == nil {
		return -1, nil, StatusInternal
	}
	defer C.free(buffer)
	var outFD C.int = -1
	var length C.uint32_t
	status := C.restic_bridge_s3_teardown_observe_v1(C.int(fd), (*C.char)(dest), C.uint32_t(len(destination)), (*C.char)(scope), C.uint32_t(len(pending)), &outFD, (*C.char)(buffer), 4096, &length)
	if status != 0 {
		return int(outFD), nil, int32(status)
	}
	if length > 4096 {
		if outFD >= 0 {
			C.close(outFD)
		}
		return -1, nil, StatusInternal
	}
	return int(outFD), C.GoBytes(buffer, C.int(length)), int32(status)
}

func teardownCRetire(fd int, destination, expected []byte) int32 {
	dest := C.CBytes(destination)
	defer C.free(dest)
	record := C.CBytes(expected)
	defer C.free(record)
	return int32(C.restic_bridge_s3_teardown_retire_v1(C.int(fd), (*C.char)(dest), C.uint32_t(len(destination)), (*C.char)(record), C.uint32_t(len(expected))))
}

func teardownCVerify(fd int, destination, expected []byte) int32 {
	dest := C.CBytes(destination)
	defer C.free(dest)
	record := C.CBytes(expected)
	defer C.free(record)
	return int32(C.restic_bridge_s3_teardown_verify_v1(C.int(fd), (*C.char)(dest), C.uint32_t(len(destination)), (*C.char)(record), C.uint32_t(len(expected))))
}

func teardownCStart(input sourceCInput, fd int, ids []string, admission []byte, reserved uint32) (uint64, int32) {
	var allocations []unsafe.Pointer
	defer func() {
		for _, p := range allocations {
			C.free(p)
		}
	}()
	request := sourceCS3Request(input, 11, &allocations)
	var joined []byte
	for _, id := range ids {
		joined = append(joined, []byte(id)...)
	}
	values := C.CBytes(joined)
	defer C.free(values)
	binding := C.CBytes(admission)
	defer C.free(binding)
	options := C.restic_operation_options_v1{version: 1, reserved1: C.uint32_t(reserved)}
	var id C.uint64_t
	status := C.restic_bridge_teardown_v1_start(nil, &request, &options, C.int(fd), (*C.char)(values), C.uint32_t(len(ids)), (*C.char)(binding), C.uint32_t(len(admission)), &id)
	return uint64(id), int32(status)
}
