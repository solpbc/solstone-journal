//go:build darwin || ios

// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

import (
	"os"
	"path/filepath"
	"testing"
)

func TestArchiveDirectoryKernelPathRetainsSelectedIdentity(t *testing.T) {
	for _, substituted := range []bool{false, true} {
		name := "renamed-before-lookup"
		if substituted {
			name = "substituted-after-lookup"
		}
		t.Run(name, func(t *testing.T) {
			parent := t.TempDir()
			selected, moved := filepath.Join(parent, "selected"), filepath.Join(parent, "moved")
			if err := os.Mkdir(selected, 0700); err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(filepath.Join(selected, "clip.m4a"), []byte("selected original"), 0600); err != nil {
				t.Fatal(err)
			}
			borrowed, err := os.Open(selected)
			if err != nil {
				t.Fatal(err)
			}
			defer borrowed.Close()
			before, err := borrowed.Stat()
			if err != nil {
				t.Fatal(err)
			}
			replace := func() {
				if err := os.Rename(selected, moved); err != nil {
					t.Fatal(err)
				}
				if err := os.Mkdir(selected, 0700); err != nil {
					t.Fatal(err)
				}
				if err := os.WriteFile(filepath.Join(selected, "clip.m4a"), []byte("unselected substitute"), 0600); err != nil {
					t.Fatal(err)
				}
			}
			if substituted {
				oldHook := testOwnedStage
				defer func() { testOwnedStage = oldHook }()
				testOwnedStage = func(at string, _ *ownedCall) {
					if at == "before-adopt-root" {
						replace()
					}
				}
			} else {
				replace()
			}
			entry, status := reserveOwnedCall()
			if status != StatusOk {
				t.Fatal(status)
			}
			defer entry.finishAdmission(StatusInvalidArgument)
			stage := archiveStartSourceRoot
			root, status := admitArchiveDirectoryDiagnosed(int(borrowed.Fd()), entry, &stage)
			if substituted {
				if root != nil || status != StatusInvalidArgument || stage != archiveStartSourceRootIdentity || entry.failed() {
					t.Fatalf("rebound path admitted or changed custody: root=%v status=%d stage=%d", root, status, stage)
				}
				for _, resource := range entry.resources {
					if !resource.started || resource.err != nil {
						t.Fatal("rejected root did not complete its existing owned close")
					}
				}
			} else {
				if root == nil || status != StatusOk {
					t.Fatalf("renamed held directory refused: status=%d stage=%d", status, stage)
				}
				data, err := root.ReadFile("clip.m4a")
				if err != nil || string(data) != "selected original" {
					t.Fatalf("root did not retain selected source: %q %v", data, err)
				}
			}
			after, err := borrowed.Stat()
			if err != nil || !os.SameFile(before, after) {
				t.Fatal("borrowed source descriptor was closed or rebound", err)
			}
			data, err := os.ReadFile(filepath.Join(selected, "clip.m4a"))
			if err != nil || string(data) != "unselected substitute" {
				t.Fatal("substituted source changed", err)
			}
		})
	}
}
