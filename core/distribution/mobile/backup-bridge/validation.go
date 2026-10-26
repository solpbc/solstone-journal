// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

type requestField struct {
	length  uint32
	present bool
}
type requestShape struct {
	operation int32
	fields    [6]requestField
}

func validLocalOperation(op int32) bool {
	switch op {
	case OpInit, OpOpen, OpAddRecoveryKey, OpSnapshot, OpListSnapshots, OpRestore, OpCheck, OpListRecoveryKeys, OpRemoveRecoveryKey, OpRetention:
		return true
	default:
		return false
	}
}

// Validate before copying any foreign memory. A nonzero length never authorizes a NULL pointer.
func validateShape(request requestShape) int32 {
	if !validLocalOperation(request.operation) {
		return StatusInvalidArgument
	}
	caps := [6]uint32{maxPasswordLen, maxPasswordLen, maxPathLen, maxPathLen, maxPathLen, idHexLen}
	for i, field := range request.fields {
		if field.length > caps[i] {
			return StatusCapExceeded
		}
		if field.length != 0 && !field.present {
			return StatusInvalidArgument
		}
	}
	if request.fields[0].length == 0 || request.fields[2].length == 0 {
		return StatusInvalidArgument
	}
	if request.operation == OpAddRecoveryKey && request.fields[1].length == 0 {
		return StatusInvalidArgument
	}
	if (request.operation == OpListRecoveryKeys || request.operation == OpRemoveRecoveryKey) && request.fields[1].length != 0 {
		return StatusInvalidArgument
	}
	if (request.operation == OpListRecoveryKeys || request.operation == OpRemoveRecoveryKey) && (request.fields[3].length != 0 || request.fields[4].length != 0 || request.fields[5].length != 0) {
		return StatusInvalidArgument
	}
	if (request.operation == OpSnapshot || request.operation == OpRestore) && request.fields[3].length == 0 {
		return StatusInvalidArgument
	}
	if request.operation == OpRestore && (request.fields[4].length == 0 || request.fields[5].length != idHexLen) {
		return StatusInvalidArgument
	}
	if request.operation == OpRetention && (request.fields[1].length != 0 || request.fields[3].length != 0 || request.fields[4].length != 0 || request.fields[5].length != 0) {
		return StatusInvalidArgument
	}
	return StatusOk
}
