// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

import (
	"fmt"
	"testing"
	"time"

	"github.com/restic/restic/internal/restic"
)

func TestOperationOptionsRejectIncompatibleAuthority(t *testing.T) {
	for _, operation := range []int32{OpInit, OpOpen, OpAddRecoveryKey, OpSnapshot, OpListSnapshots, OpCheck, OpRestore, OpS3Inspect, OpListRecoveryKeys, OpRemoveRecoveryKey, OpRetention, -1, 99} {
		for _, mode := range []uint32{checkReadNone, checkReadStructure, checkReadAll, checkReadGroup, 4, ^uint32(0)} {
			for _, pair := range [][2]uint32{{0, 0}, {1, 1}, {1, 52}, {52, 52}, {256, 256}, {0, 1}, {1, 0}, {53, 52}, {1, 257}} {
				for _, retry := range []uint64{0, 1, maxRetryLockMillis, maxRetryLockMillis + 1, ^uint64(0)} {
					name := fmt.Sprintf("op%d/mode%d/group%dof%d/retry%d", operation, mode, pair[0], pair[1], retry)
					t.Run(name, func(t *testing.T) {
						got, status := parseOperationOptions(operation, 1, mode, retry, pair[0], pair[1], 0, 0)
						validOperation := operation == OpInit || operation == OpOpen || operation == OpAddRecoveryKey || operation == OpSnapshot || operation == OpListSnapshots || operation == OpCheck || operation == OpS3Inspect || operation == OpListRecoveryKeys || operation == OpRemoveRecoveryKey || operation == OpRetention
						validRetry := retry <= maxRetryLockMillis && (retry == 0 || operation == OpAddRecoveryKey || operation == OpSnapshot || operation == OpListSnapshots || operation == OpCheck || operation == OpRemoveRecoveryKey || operation == OpRetention)
						validMode := mode == checkReadNone && pair == [2]uint32{0, 0}
						if operation == OpCheck {
							validMode = ((mode == checkReadStructure || mode == checkReadAll) && pair == [2]uint32{0, 0}) || (mode == checkReadGroup && pair[0] > 0 && pair[0] <= pair[1] && pair[1] <= 256)
						}
						want := validOperation && validRetry && validMode
						if (status == StatusOk) != want || (got != nil) != want {
							t.Fatal("option compatibility mismatch", want, status, got)
						}
						if want && (got.retryLock != time.Duration(retry)*time.Millisecond || got.checkMode != mode || got.group != pair[0] || got.groups != pair[1]) {
							t.Fatal("validated owned copy changed options")
						}
					})
				}
			}
		}
	}
	// Version 2 adds only the append-only bit in reserved1.
	for _, fields := range [][3]uint32{{0, 0, 0}, {3, 0, 0}, {1, 1, 0}, {1, 0, 1}, {2, 2, 0}, {2, 0, 1}} {
		if got, status := parseOperationOptions(OpOpen, fields[0], checkReadNone, 0, 0, 0, fields[1], fields[2]); got != nil || status != StatusInvalidArgument {
			t.Fatal("version/reserved field accepted", fields)
		}
	}
}

func TestOperationOptionsGroupMembershipAndEmptyGroup(t *testing.T) {
	packs := make(map[restic.ID]int64)
	for first := 0; first < 256; first++ {
		var id restic.ID
		id[0], id[31] = byte(first), byte(255-first)
		packs[id] = int64(first + 100)
	}
	for _, groups := range []uint32{1, 2, 3, 52, 255, 256} {
		seen := make(map[restic.ID]int)
		for group := uint32(1); group <= groups; group++ {
			selected := checkPackFilter(&operationOptions{checkMode: checkReadGroup, group: group, groups: groups})(packs)
			for id, size := range packs {
				// Independent membership construction: the arithmetic progression of
				// possible first bytes, rather than repeating production modulo.
				want := false
				for first := int(group - 1); first < 256; first += int(groups) {
					want = want || int(id[0]) == first
				}
				value, got := selected[id]
				if got != want || (got && value != size) {
					t.Fatal("group membership or size mismatch", groups, group, id)
				}
				if got {
					seen[id]++
				}
			}
		}
		for id := range packs {
			if seen[id] != 1 {
				t.Fatal("partition omitted or repeated pack", groups, id, seen[id])
			}
		}
	}
	if checkPackFilter(&operationOptions{checkMode: checkReadStructure}) != nil {
		t.Fatal("structure mode scheduled pack-content verification")
	}
	for _, options := range []*operationOptions{nil, {checkMode: checkReadAll}} {
		if len(checkPackFilter(options)(packs)) != len(packs) {
			t.Fatal("legacy/full mode omitted packs")
		}
	}
	var only restic.ID
	if selected := checkPackFilter(&operationOptions{checkMode: checkReadGroup, group: 256, groups: 256})(map[restic.ID]int64{only: 100}); len(selected) != 0 {
		t.Fatal("empty group fabricated pack reads")
	}
}
