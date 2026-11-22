//go:build integration

// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"testing"
)

func TestSnapshotSelectionActualEncryptedRestorePreservesOwnerFiles(t *testing.T) {
	ctx := context.Background()
	base := t.TempDir()
	repoPath := filepath.Join(base, "repo")
	sourcePath := filepath.Join(base, `journal[*]?\$界`)
	for _, path := range []string{repoPath, sourcePath} {
		if err := os.Mkdir(path, 0700); err != nil {
			t.Fatal(err)
		}
	}
	kept := map[string]string{
		"owner.txt":                                           "owner record",
		"owner/cache/record.txt":                              "owner directory named cache",
		"owner/config/chatgpt-sign-in.json":                   "owner nested filename",
		"owner/config/plaud-sign-in.json":                     "owner nested service credential name",
		"config/sign-in.json":                                 "non-service sign-in file",
		"health/durable.json":                                 "durable health record",
		"imports/health-dedupe.sqlite/owner.txt":              "owner directory, not a database file",
		"chronicle/20261015/indexer/120000_60.sqlite":         "owner raw recording name",
		"chronicle/20261015/indexer/indexer.sqlite/owner.txt": "owner directory in mixed namespace",
	}
	excluded := map[string]string{
		"cache/private.txt":                                 "excluded cache payload",
		"config/chatgpt-sign-in.json":                       "excluded credential payload",
		"config/chatgpt-sign-in.json.tmp":                   "excluded credential candidate",
		"config/plaud-sign-in.json":                         "excluded service credential payload",
		"config/plaud-sign-in.json.lock":                    "excluded service credential lock",
		"config/plaud-sign-in.json.renewal":                 "excluded service credential renewal",
		"config/plaud-sign-in.json.unreadable-1":            "excluded unreadable service credential",
		"health/nested/.removing_123/record.txt":            "excluded deletion pivot",
		"imports/.health-dedupe.sqlite.rebuild":             "excluded live database",
		"imports/.health-dedupe.sqlite.rebuild-wal":         "excluded live database WAL",
		"chronicle/20261015/indexer/transcripts.sqlite-shm": "excluded day database SHM",
	}
	for _, files := range []map[string]string{kept, excluded} {
		for relative, content := range files {
			path := filepath.Join(sourcePath, relative)
			if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(path, []byte(content), 0600); err != nil {
				t.Fatal(err)
			}
		}
	}
	policy, _ := json.Marshal([]string{"cache", "config/*-sign-in.json*", "**/.removing_*",
		"imports/.health-dedupe.sqlite.rebuild", "imports/.health-dedupe.sqlite.rebuild-wal",
		"chronicle/20261015/indexer/transcripts.sqlite-shm"})
	selection, status := parseSnapshotSelection(policy)
	if status != StatusOk {
		t.Fatal("policy refused", status)
	}
	password := []byte("snapshot-selection-synthetic-key")
	if result, err := executeInit(ctx, bridgeRequest{operation: OpInit, repoPath: repoPath, password: password}, func() {}); err != nil || result.status != StatusOk {
		t.Fatal("init failed", result.status, err)
	}
	result, err := executeSnapshot(ctx, bridgeRequest{operation: OpSnapshot, repoPath: repoPath, sourcePath: sourcePath, password: password, snapshotSelection: selection}, func() {})
	if err != nil || result.status != StatusOk {
		t.Fatal("snapshot failed", result.status, err)
	}
	restorePath := filepath.Join(base, "restored")
	restoreReq := bridgeRequest{operation: OpRestore, repoPath: repoPath, sourcePath: sourcePath, password: password,
		restorePath: restorePath, snapshotID: result.snapshotID}
	if restored, err := executeRestore(ctx, &restoreReq, func() {}); err != nil || restored.status != StatusOk {
		t.Fatal("restore failed", restored.status, err)
	}
	restoredRoot := filepath.Join(restorePath, filepath.Base(sourcePath))
	actual := map[string]string{}
	err = filepath.WalkDir(restoredRoot, func(path string, entry os.DirEntry, err error) error {
		if err != nil || entry.IsDir() {
			return err
		}
		content, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		relative, err := filepath.Rel(restoredRoot, path)
		if err != nil {
			return err
		}
		actual[relative] = string(content)
		return nil
	})
	if err != nil || !reflect.DeepEqual(actual, kept) {
		t.Fatal("restored file set/bytes changed", err, actual)
	}
	for _, files := range []map[string]string{kept, excluded} {
		for relative, expected := range files {
			actual, err := os.ReadFile(filepath.Join(sourcePath, relative))
			if err != nil || string(actual) != expected {
				t.Fatal("snapshot modified source", relative, err)
			}
		}
	}
}
