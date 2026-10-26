// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

import "testing"

func s3ShapeFor(operation int32) [14]requestField {
	var fields [14]requestField
	for _, index := range []int{0, 1, 2, 3, 4, 5, 9} {
		fields[index] = requestField{1, true}
	}
	if operation != OpS3Inspect {
		fields[7] = requestField{1, true}
	}
	if operation == OpAddRecoveryKey {
		fields[8] = requestField{1, true}
	}
	if operation == OpSnapshot || operation == OpRestore {
		fields[10] = requestField{1, true}
	}
	if operation == OpRestore {
		fields[11], fields[12] = requestField{1, true}, requestField{idHexLen, true}
	}
	if operation == OpSnapshot {
		fields[13] = requestField{1, true}
	}
	return fields
}

func TestS3RequestShapeRejectsBeforeForeignCopy(t *testing.T) {
	for operation := OpInit; operation <= OpRemoveRecoveryKey; operation++ {
		valid := s3ShapeFor(operation)
		if validateS3Shape(1, operation, valid) != StatusOk {
			t.Fatal("supported request refused")
		}
		for index := range valid {
			bad := valid
			bad[index] = requestField{1, false}
			if validateS3Shape(1, operation, bad) != StatusInvalidArgument {
				t.Fatal("nonzero foreign length authorized NULL")
			}
			bad[index] = requestField{^uint32(0), true}
			if validateS3Shape(1, operation, bad) != StatusCapExceeded {
				t.Fatal("huge foreign length escaped cap")
			}
		}
		for _, index := range []int{0, 1, 2, 3, 4, 5, 9} {
			bad := valid
			bad[index] = requestField{}
			if validateS3Shape(1, operation, bad) != StatusInvalidArgument {
				t.Fatal("missing authority/state field accepted")
			}
		}
	}
	if validateS3Shape(2, OpInit, s3ShapeFor(OpInit)) != StatusInvalidArgument || validateS3Shape(1, 11, s3ShapeFor(OpInit)) != StatusInvalidArgument {
		t.Fatal("unknown version/operation accepted")
	}
	for _, operation := range []int32{OpOpen, OpListSnapshots, OpCheck, OpS3Inspect, OpListRecoveryKeys, OpRemoveRecoveryKey} {
		bad := s3ShapeFor(operation)
		bad[13] = requestField{1, true}
		if validateS3Shape(1, operation, bad) != StatusInvalidArgument {
			t.Fatal("unused workspace field accepted")
		}
	}
	retentionValid := s3ShapeFor(OpCheck)
	if validateS3Shape(1, OpRetention, retentionValid) != StatusOk {
		t.Fatal("OpRetention check-like shape refused")
	}
	badSource := retentionValid
	badSource[10] = requestField{1, true}
	if validateS3Shape(1, OpRetention, badSource) != StatusInvalidArgument {
		t.Fatal("OpRetention source field accepted")
	}
	badWorkspace := retentionValid
	badWorkspace[13] = requestField{1, true}
	if validateS3Shape(1, OpRetention, badWorkspace) != StatusInvalidArgument {
		t.Fatal("OpRetention workspace field accepted")
	}
	if validateS3Shape(1, 13, retentionValid) != StatusInvalidArgument || validateS3Shape(1, 20, retentionValid) != StatusInvalidArgument {
		t.Fatal("unassigned opcodes 13 or 20 accepted")
	}
}
