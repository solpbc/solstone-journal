// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

import "os"

// Numerical caps enforced on requests and responses.
const (
	maxPasswordLen   = 256  // daily key is 43 base64url chars, recovery key is 64 Crockford chars
	maxPathLen       = 4096 // POSIX PATH_MAX
	maxSnapshotCount = 4096 // maximum snapshots returned in one call
	idHexLen         = 64   // restic ID is 32 bytes (64 lowercase hex characters)
	maxDiagnosticLen = 256  // stable diagnostic buffer length
)

// Status codes returned by the bridge.
const (
	StatusOk                       int32 = 0
	StatusInvalidArgument          int32 = 1
	StatusFailed                   int32 = 2
	StatusCancelled                int32 = 3
	StatusCancelledUncertain       int32 = 4
	StatusCapExceeded              int32 = 5
	StatusBusy                     int32 = 6
	StatusInvalidHandle            int32 = 7
	StatusInternal                 int32 = 8
	StatusCommittedUncertain       int32 = 9
	StatusWorkspaceCleanupRequired int32 = 10
)

// Closed operation codes.
const (
	OpInit              int32 = 1
	OpOpen              int32 = 2
	OpAddRecoveryKey    int32 = 3
	OpSnapshot          int32 = 4
	OpListSnapshots     int32 = 5
	OpRestore           int32 = 6
	OpCheck             int32 = 7
	OpListRecoveryKeys  int32 = 9
	OpRemoveRecoveryKey int32 = 10
	// Closed archive effects. Whole-journal snapshot and restore stay 4 and 6.
	// Teardown stays 11. 12 is not an archive effect.
	OpArchiveSources int32 = 20
	OpListNodes      int32 = 21
	OpReadFile       int32 = 22
	OpRetention      int32 = 12
)

// Closed set of stable diagnostic tokens. Raw upstream strings containing passwords
// or secret keys are dropped and mapped to these tokens.
const (
	DiagOk                       = ""
	DiagInvalidArgument          = "INVALID_ARGUMENT"
	DiagCapExceeded              = "CAP_EXCEEDED"
	DiagAuthFailed               = "AUTH_FAILED"
	DiagRepoMissing              = "REPO_MISSING"
	DiagRepoExists               = "REPO_EXISTS"
	DiagRepoCorrupt              = "REPO_CORRUPT"
	DiagIoFailed                 = "IO_FAILED"
	DiagLocked                   = "REPO_LOCKED"
	DiagCancelled                = "CANCELLED"
	DiagCancelledUncertain       = "CANCELLED_UNCERTAIN"
	DiagSnapshotEmpty            = "SNAPSHOT_EMPTY"
	DiagRestoreNonEmpty          = "RESTORE_DESTINATION_NOT_EMPTY"
	DiagRestoreDestinationExists = "RESTORE_DESTINATION_EXISTS"
	DiagRestoreDestinationWrong  = "RESTORE_DESTINATION_INVALID"
	DiagInternal                 = "INTERNAL_ERROR"
	DiagCommittedUncertain       = "COMMITTED_UNCERTAIN"
	DiagWorkspaceCleanupRequired = "WORKSPACE_CLEANUP_REQUIRED"
	DiagIncompleteDiscovery      = "INCOMPLETE_DISCOVERY"
	DiagSnapshotNodeUnavailable  = "SNAPSHOT_NODE_UNAVAILABLE"
)

// Internal Go representation of caller request parameters.
type bridgeRequest struct {
	owned               *ownedCall
	operation           int32
	password            []byte
	recoveryPassword    []byte
	repoPath            string
	sourcePath          string
	sourceName          string
	restorePath         string
	snapshotID          string
	removeKeyID         string
	maintenanceRotation bool
	rotationCustody     bool
	snapshotSelection   *snapshotSelection
	retentionPolicy     *retentionPolicy
	executionOptions    *operationOptions
	// pathFiles pin caller-authorized directories for the complete asynchronous
	// operation.
	pathFiles            []*os.File
	metadataPath         string
	metadataRoot         *os.Root
	metadataFile         *os.File
	repoRoot             *os.Root
	sourceRoot           *os.Root
	restoreRoot          *os.Root
	restoreParentPath    string
	restoreParent        *os.File
	restoreStageRoot     *os.File
	restoreStage         *os.File
	restoreName          string
	restoreStageRootName string
	restoreStageName     string
	restorePublished     bool
	pathsPinned          bool
	remote               *s3RemoteRequest
	teardownIDs          []string
	teardownAdmission    *s3TeardownAdmission
	rootedRestore        *rootedRestore
	archiveNames         []string
	archiveNodePath      string
	archiveTarget        *os.File
	archiveSizeBound     uint64
}

// Internal Go representation of call results.
type bridgeResult struct {
	status        int32
	keyID         string
	snapshotID    string
	snapshotCount uint32
	snapshotIDs   []string
	diagnostic    string
	// Optional read metadata has its own bounded-output outcome. Failure to
	// encode metadata never changes an existing operation's legacy result.
	metadata       []byte
	metadataStatus int32
	recoveryResume bool
}
