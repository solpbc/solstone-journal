// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

import (
	"bytes"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"golang.org/x/sys/unix"
)

func TestArchiveRootOpenRefusalIsBounded(t *testing.T) {
	for _, control := range []struct {
		err   error
		stage archiveStartStage
	}{
		{unix.EACCES, archiveStartSourceRootOpenDenied},
		{unix.EPERM, archiveStartSourceRootOpenDenied},
		{unix.ENOENT, archiveStartSourceRootOpenUnavailable},
		{unix.ENOTDIR, archiveStartSourceRootOpenUnavailable},
		{unix.EMFILE, archiveStartSourceRootOpenResource},
		{unix.ENFILE, archiveStartSourceRootOpenResource},
		{unix.ENOMEM, archiveStartSourceRootOpenResource},
		{unix.EIO, archiveStartSourceRootOpenFailed},
	} {
		wrapped := &os.PathError{Op: "open", Path: "private synthetic argument", Err: control.err}
		if got := archiveRootOpenRefusal(wrapped); got != control.stage {
			t.Fatalf("wrapped native refusal misclassified: %d != %d", got, control.stage)
		}
	}
}

func TestArchiveRootAdmissionRefusalRetainsChecksAndCaller(t *testing.T) {
	for _, kind := range []string{"missing", "not-directory", "identity", "pinned-stat"} {
		t.Run(kind, func(t *testing.T) {
			selected, other := t.TempDir(), t.TempDir()
			pinned, err := os.Open(selected)
			if err != nil {
				t.Fatal(err)
			}
			defer pinned.Close()
			path, want := other, archiveStartSourceRootIdentity
			switch kind {
			case "missing":
				path, want = filepath.Join(other, "absent"), archiveStartSourceRootOpenUnavailable
			case "not-directory":
				file := filepath.Join(other, "file")
				if err := os.WriteFile(file, []byte("preserved"), 0600); err != nil {
					t.Fatal(err)
				}
				path, want = filepath.Join(file, "child"), archiveStartSourceRootOpenUnavailable
			case "pinned-stat":
				path, want = selected, archiveStartSourceRootPinnedStat
				if err := pinned.Close(); err != nil {
					t.Fatal(err)
				}
			}
			entry, status := reserveOwnedCall()
			if status != StatusOk {
				t.Fatal(status)
			}
			defer entry.finishAdmission(StatusInvalidArgument)
			stage := archiveStartSourceRoot
			root, err := adoptOwnedRootDiagnosed(path, pinned, entry, &stage)
			if root != nil || !errors.Is(err, errPinnedIdentity) || stage != want || entry.failed() {
				t.Fatalf("refusal changed error/custody: root=%v err=%v stage=%d want=%d", root, err, stage, want)
			}
			if kind != "pinned-stat" {
				if _, err := pinned.Stat(); err != nil {
					t.Fatal("caller descriptor closed by refusal", err)
				}
			}
			for _, closed := range entry.resources {
				if !closed.started || closed.err != nil {
					t.Fatal("rejected root did not complete its existing close")
				}
			}
		})
	}
}

func TestArchiveDirectoryAdmissionRetainsBorrowedSource(t *testing.T) {
	directory := t.TempDir()
	if err := os.WriteFile(filepath.Join(directory, "clip.m4a"), []byte("retained source"), 0600); err != nil {
		t.Fatal(err)
	}
	borrowed, err := os.Open(directory)
	if err != nil {
		t.Fatal(err)
	}
	defer borrowed.Close()
	entry, status := reserveOwnedCall()
	if status != StatusOk {
		t.Fatal(status)
	}
	defer entry.finishAdmission(StatusInvalidArgument)
	root, status := admitArchiveDirectory(int(borrowed.Fd()), entry)
	if status != StatusOk {
		t.Fatalf("ordinary retained source descriptor refused: %d", status)
	}
	data, err := root.ReadFile("clip.m4a")
	if err != nil || string(data) != "retained source" {
		t.Fatalf("admitted root did not retain the selected source: %q %v", data, err)
	}
	if _, err := borrowed.Stat(); err != nil {
		t.Fatal("caller descriptor was closed", err)
	}
}

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
