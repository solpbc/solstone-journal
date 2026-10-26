// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

import (
	"context"
	"time"

	"github.com/restic/restic/internal/repository"
	"github.com/restic/restic/internal/restic"
)

const (
	checkReadNone      uint32 = 0
	checkReadStructure uint32 = 1
	checkReadAll       uint32 = 2
	checkReadGroup     uint32 = 3
	maxRetryLockMillis uint64 = 48 * 60 * 60 * 1000
)

// A validated owned copy, never a pointer into caller C storage. Legacy requests
// carry nil options: no repository-lock retry and the historical full check.
type operationOptions struct {
	retryLock time.Duration
	checkMode uint32
	group     uint32
	groups    uint32
	// appendOnly makes every non-lock S3 write create-only (version 2 only).
	appendOnly bool
}

func parseOperationOptions(operation int32, version, mode uint32, retryMillis uint64, group, groups, reserved1, reserved2 uint32) (*operationOptions, int32) {
	// Version 2 adds reserved1 bit 0: an append-only (create-only) destination.
	if version < 1 || version > 2 || reserved2 != 0 || reserved1 > 1 || version == 1 && reserved1 != 0 {
		return nil, StatusInvalidArgument
	}
	if retryMillis > maxRetryLockMillis || groups > 256 {
		return nil, StatusCapExceeded
	}
	switch operation {
	case OpInit, OpOpen, OpS3Inspect, OpListRecoveryKeys:
		if retryMillis != 0 {
			return nil, StatusInvalidArgument
		}
	case OpAddRecoveryKey, OpSnapshot, OpListSnapshots, OpCheck, OpRemoveRecoveryKey, OpRetention:
	default:
		return nil, StatusInvalidArgument
	}
	if operation != OpCheck {
		if mode != checkReadNone || group != 0 || groups != 0 {
			return nil, StatusInvalidArgument
		}
	} else {
		switch mode {
		case checkReadStructure, checkReadAll:
			if group != 0 || groups != 0 {
				return nil, StatusInvalidArgument
			}
		case checkReadGroup:
			if group == 0 || groups == 0 || group > groups {
				return nil, StatusInvalidArgument
			}
		default:
			return nil, StatusInvalidArgument
		}
	}
	return &operationOptions{retryLock: time.Duration(retryMillis) * time.Millisecond, checkMode: mode, group: group, groups: groups, appendOnly: reserved1 == 1}, StatusOk
}

// Keep the pinned SDK's retry-budget semantics, including its final acquisition
// attempt. This timer does not impose a deadline on already-acquired operation
// work; the actual operation context remains the cancellation authority.
func lockRepoForRequest(ctx context.Context, req bridgeRequest, repo *repository.Repository, exclusive bool) (func(), context.Context, error) {
	retry := time.Duration(0)
	if req.executionOptions != nil {
		retry = req.executionOptions.retryLock
	}
	return lockRepoWithRetry(ctx, repo, exclusive, retry)
}

func lockRepoWithRetry(ctx context.Context, repo *repository.Repository, exclusive bool, retry time.Duration) (func(), context.Context, error) {
	printRetry := func(_ string) {}
	logger := func(_ string, _ ...interface{}) {}
	unlocker, lockCtx, err := repository.Lock(ctx, repo, exclusive, retry, printRetry, logger)
	if err != nil {
		return nil, ctx, err
	}
	return unlocker.Unlock, lockCtx, nil
}

func checkPackFilter(options *operationOptions) func(map[restic.ID]int64) map[restic.ID]int64 {
	if options == nil || options.checkMode == checkReadAll {
		return func(packs map[restic.ID]int64) map[restic.ID]int64 { return packs }
	}
	if options.checkMode == checkReadStructure {
		return nil
	}
	return func(all map[restic.ID]int64) map[restic.ID]int64 {
		selected := make(map[restic.ID]int64)
		for id, size := range all {
			if uint32(id[0])%options.groups+1 == options.group {
				selected[id] = size
			}
		}
		return selected
	}
}

// A private deterministic test observer can release the actual acquired lock.
// No production callback/FFI setter installs it. The caller waits for each
// checker phase before invoking it, so lock loss cannot masquerade as clean EOF.
var testCheckPhase func(string, context.Context, func())

func checkContextsAfterPhase(parent, acquired context.Context, phase string, unlock func()) (bridgeResult, error) {
	if testCheckPhase != nil {
		testCheckPhase(phase, acquired, unlock)
	}
	if err := parent.Err(); err != nil {
		return bridgeResult{status: StatusCancelled, diagnostic: DiagCancelled}, err
	}
	if err := acquired.Err(); err != nil {
		return bridgeResult{status: StatusFailed, diagnostic: DiagLocked}, err
	}
	return bridgeResult{}, nil
}
