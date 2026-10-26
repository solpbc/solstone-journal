// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

import (
	"bytes"
	"errors"
	"strings"
	"testing"
)

func TestArchiveOpcodesStayOutsideOrdinarySnapshotRestoreAndRetention(t *testing.T) {
	ops := []int32{OpArchiveSources, OpListNodes, OpReadFile}
	for _, op := range ops {
		if op < 20 || op == OpSnapshot || op == OpRestore || op == OpTeardown || op == OpS3Inspect || op == 8 || op == 12 {
			t.Fatalf("archive opcode %d collides with an existing effect", op)
		}
		if validLocalOperation(op) || normalDirectoryOperation(op) {
			t.Fatalf("archive opcode %d is admitted as an ordinary directory operation", op)
		}
		if !validOwnerOperation(op) {
			t.Fatalf("archive opcode %d is not an owner operation", op)
		}
	}
	if !adoptsUncertainty(OpArchiveSources) || adoptsUncertainty(OpListNodes) || adoptsUncertainty(OpReadFile) || adoptsUncertainty(OpTeardown) {
		t.Fatal("uncertainty follows the archive write only")
	}
}

func TestParseArchiveNamesAcceptsExactRelativeFilesOnly(t *testing.T) {
	names, ok := parseArchiveNames([]byte(`["clip.m4a","nested/take.m4a"]`))
	if !ok || len(names) != 2 || names[0] != "clip.m4a" || names[1] != "nested/take.m4a" {
		t.Fatalf("exact names rejected: %v %v", names, ok)
	}
	rejected := [][]byte{
		nil,
		[]byte(`[]`),
		[]byte(`["clip.m4a","clip.m4a"]`),
		[]byte(`["/clip.m4a"]`),
		[]byte(`["../clip.m4a"]`),
		[]byte(`["a/../b"]`),
		[]byte(`["a//b"]`),
		[]byte(`["."]`),
		[]byte(`[""]`),
		[]byte(`["clip.m4a"] true`),
		[]byte(`{"name":"clip.m4a"}`),
		[]byte(`["clip.m4a"`),
	}
	for _, payload := range rejected {
		if _, ok := parseArchiveNames(payload); ok {
			t.Fatalf("malformed names accepted: %s", payload)
		}
	}
}

func TestNodeListingAccountsForEveryNodeOrRefuses(t *testing.T) {
	populated := []archiveNodeJSON{
		{MessageType: "node", Type: "dir", Path: "retained", Name: "retained", Size: 0},
		{MessageType: "node", Type: "file", Path: "retained/clip.m4a", Name: "clip.m4a", Size: 4},
	}
	result := finishNodeListing(populated, nil)
	if result.status != StatusOk || result.metadataStatus != StatusOk || len(result.metadata) == 0 {
		t.Fatalf("complete listing refused: %+v", result)
	}
	if !bytes.Contains(result.metadata, []byte(`"message_type":"node"`)) || !bytes.Contains(result.metadata, []byte(`"node_count":2`)) {
		t.Fatalf("listing shape drifted: %s", result.metadata)
	}
	empty := finishNodeListing(nil, nil)
	if empty.status != StatusOk || !bytes.Contains(empty.metadata, []byte(`"complete":true`)) || !bytes.Contains(empty.metadata, []byte(`"node_count":0`)) {
		t.Fatalf("complete empty listing refused: %+v %s", empty, empty.metadata)
	}
	walk := finishNodeListing(populated, errors.New("walk failed"))
	if walk.status != StatusFailed || walk.diagnostic != DiagIncompleteDiscovery || walk.metadata != nil {
		t.Fatalf("walk error returned nodes: %+v", walk)
	}
	huge := finishNodeListing([]archiveNodeJSON{{
		MessageType: "node",
		Type:        "file",
		Path:        strings.Repeat("a", maxNodeListingBytes+1),
		Name:        "a",
		Size:        1,
	}}, nil)
	if huge.status != StatusCapExceeded || huge.metadata != nil {
		t.Fatalf("cap returned a listing: %+v", huge)
	}
	missing := finishNodeListing([]archiveNodeJSON{{MessageType: "node", Type: "file", Path: "a", Name: "", Size: 1}}, nil)
	if missing.status != StatusFailed || missing.diagnostic != DiagIncompleteDiscovery || missing.metadata != nil {
		t.Fatalf("malformed node became a listing: %+v", missing)
	}
}
