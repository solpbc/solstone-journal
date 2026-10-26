//go:build integration

// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

import (
	"bytes"
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

func archiveTestRepository(t *testing.T) (string, []byte) {
	t.Helper()
	repo := filepath.Join(t.TempDir(), "repo")
	if err := os.Mkdir(repo, 0700); err != nil {
		t.Fatal(err)
	}
	password := []byte("archive-fixture-password")
	req := bridgeRequest{operation: OpInit, password: append([]byte(nil), password...), repoPath: repo}
	if err := pinRequestPaths(&req); err != nil {
		t.Fatal(err)
	}
	result, err := executeInit(context.Background(), req, func() {})
	closeRequestPaths(&req)
	if err != nil || result.status != StatusOk {
		t.Fatalf("init failed: %+v %v", result, err)
	}
	return repo, password
}

func archiveRetainedFile(t *testing.T, repo string, password []byte, logical string, sourceDir string, names []string) string {
	t.Helper()
	root, err := os.OpenRoot(sourceDir)
	if err != nil {
		t.Fatal(err)
	}
	req := bridgeRequest{
		operation:    OpArchiveSources,
		password:     append([]byte(nil), password...),
		repoPath:     repo,
		sourceName:   logical,
		sourceRoot:   root,
		archiveNames: append([]string(nil), names...),
	}
	if err := pinRequestPaths(&req); err != nil {
		_ = root.Close()
		t.Fatal(err)
	}
	result, err := executeArchiveSources(context.Background(), req, func() {})
	closeRequestPaths(&req)
	if err != nil || result.status != StatusOk || len(result.snapshotID) != idHexLen {
		t.Fatalf("archive failed: %+v %v", result, err)
	}
	return result.snapshotID
}

func archiveList(t *testing.T, repo, password, snapshot string) []archiveNodeJSON {
	t.Helper()
	req := bridgeRequest{
		operation:  OpListNodes,
		password:   []byte(password),
		repoPath:   repo,
		snapshotID: snapshot,
	}
	if err := pinRequestPaths(&req); err != nil {
		t.Fatal(err)
	}
	result, err := executeListNodes(context.Background(), req)
	closeRequestPaths(&req)
	if err != nil || result.status != StatusOk {
		t.Fatalf("list failed: %+v %v", result, err)
	}
	var nodes []archiveNodeJSON
	dec := json.NewDecoder(bytes.NewReader(result.metadata))
	for dec.More() {
		var node archiveNodeJSON
		if err := dec.Decode(&node); err != nil {
			t.Fatal(err)
		}
		if node.MessageType == "node" {
			nodes = append(nodes, node)
		}
	}
	return nodes
}

func archiveDownload(t *testing.T, repo, password, snapshot, nodePath string, bound uint64) []byte {
	t.Helper()
	dir := t.TempDir()
	targetPath := filepath.Join(dir, "issued")
	if err := os.WriteFile(targetPath, nil, 0600); err != nil {
		t.Fatal(err)
	}
	file, err := os.OpenFile(targetPath, os.O_RDWR, 0600)
	if err != nil {
		t.Fatal(err)
	}
	req := bridgeRequest{
		operation:        OpReadFile,
		password:         []byte(password),
		repoPath:         repo,
		snapshotID:       snapshot,
		archiveNodePath:  nodePath,
		archiveTarget:    file,
		archiveSizeBound: bound,
	}
	if err := pinRequestPaths(&req); err != nil {
		_ = file.Close()
		t.Fatal(err)
	}
	result, err := executeReadFile(context.Background(), req, func() {})
	closeRequestPaths(&req)
	if err != nil || result.status != StatusOk {
		t.Fatalf("download failed: %+v %v", result, err)
	}
	var drained struct {
		Drained uint64 `json:"drained"`
	}
	if json.Unmarshal(result.metadata, &drained) != nil || drained.Drained != uint64(len(mustRead(t, targetPath))) {
		t.Fatalf("drain does not match issued bytes: %+v", result.metadata)
	}
	return mustRead(t, targetPath)
}

func mustRead(t *testing.T, path string) []byte {
	t.Helper()
	buf, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return buf
}

func TestArchiveRetainedSourcesRoundTripExactBytes(t *testing.T) {
	repo, password := archiveTestRepository(t)
	source := filepath.Join(t.TempDir(), "stage")
	if err := os.Mkdir(source, 0700); err != nil {
		t.Fatal(err)
	}
	payload := []byte("kept")
	if err := os.WriteFile(filepath.Join(source, "clip.m4a"), payload, 0600); err != nil {
		t.Fatal(err)
	}
	snapshot := archiveRetainedFile(t, repo, password, "retained", source, []string{"clip.m4a"})
	nodes := archiveList(t, repo, string(password), snapshot)
	found := false
	for _, node := range nodes {
		if node.Type == "file" && node.Path == "retained/clip.m4a" && node.Name == "clip.m4a" && node.Size == uint64(len(payload)) {
			found = true
		}
	}
	if !found {
		t.Fatalf("listing missed the retained file: %+v", nodes)
	}
	got := archiveDownload(t, repo, string(password), snapshot, "retained/clip.m4a", uint64(len(payload)))
	if !bytes.Equal(got, payload) {
		t.Fatalf("downloaded %#v", got)
	}
	short := archiveDownloadExpect(t, repo, string(password), snapshot, "retained/clip.m4a", 1)
	if short.status == StatusOk {
		t.Fatal("download above the size bound succeeded")
	}
}

func archiveDownloadExpect(t *testing.T, repo, password, snapshot, nodePath string, bound uint64) bridgeResult {
	t.Helper()
	targetPath := filepath.Join(t.TempDir(), "issued")
	if err := os.WriteFile(targetPath, nil, 0600); err != nil {
		t.Fatal(err)
	}
	file, err := os.OpenFile(targetPath, os.O_RDWR, 0600)
	if err != nil {
		t.Fatal(err)
	}
	req := bridgeRequest{
		operation:        OpReadFile,
		password:         []byte(password),
		repoPath:         repo,
		snapshotID:       snapshot,
		archiveNodePath:  nodePath,
		archiveTarget:    file,
		archiveSizeBound: bound,
	}
	if err := pinRequestPaths(&req); err != nil {
		_ = file.Close()
		t.Fatal(err)
	}
	result, _ := executeReadFile(context.Background(), req, func() {})
	closeRequestPaths(&req)
	return result
}

func TestArchiveRereadsChangedBytesWithTheSameMtimeAndSize(t *testing.T) {
	repo, password := archiveTestRepository(t)
	source := filepath.Join(t.TempDir(), "stage")
	if err := os.Mkdir(source, 0700); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(source, "clip.m4a")
	if err := os.WriteFile(path, []byte("aaaa"), 0600); err != nil {
		t.Fatal(err)
	}
	info, err := os.Stat(path)
	if err != nil {
		t.Fatal(err)
	}
	first := archiveRetainedFile(t, repo, password, "retained", source, []string{"clip.m4a"})
	if err := os.WriteFile(path, []byte("bbbb"), 0600); err != nil {
		t.Fatal(err)
	}
	if err := os.Chtimes(path, info.ModTime(), info.ModTime()); err != nil {
		t.Fatal(err)
	}
	after, err := os.Stat(path)
	if err != nil || after.Size() != info.Size() || !after.ModTime().Equal(info.ModTime()) {
		t.Fatalf("same-mtime fixture drifted: before=%v after=%v", info.ModTime(), after.ModTime())
	}
	second := archiveRetainedFile(t, repo, password, "retained", source, []string{"clip.m4a"})
	if second == first {
		t.Fatal("changed bytes reused the previous snapshot id")
	}
	got := archiveDownload(t, repo, string(password), second, "retained/clip.m4a", 4)
	if !bytes.Equal(got, []byte("bbbb")) {
		t.Fatalf("reread returned %#v", got)
	}
}

func TestArchiveSourceReplacementAndTargetSubstitution(t *testing.T) {
	repo, password := archiveTestRepository(t)
	source := filepath.Join(t.TempDir(), "stage")
	if err := os.Mkdir(source, 0700); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(source, "clip.m4a")
	if err := os.WriteFile(path, []byte("old-bytes"), 0600); err != nil {
		t.Fatal(err)
	}
	first := archiveRetainedFile(t, repo, password, "retained", source, []string{"clip.m4a"})
	if err := os.Remove(path); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte("new-bytes"), 0600); err != nil {
		t.Fatal(err)
	}
	second := archiveRetainedFile(t, repo, password, "retained", source, []string{"clip.m4a"})
	if bytes.Equal(archiveDownload(t, repo, string(password), first, "retained/clip.m4a", 64), archiveDownload(t, repo, string(password), second, "retained/clip.m4a", 64)) {
		t.Fatal("source replacement did not change the archived bytes")
	}

	dir := t.TempDir()
	original := filepath.Join(dir, "issued")
	if err := os.WriteFile(original, nil, 0600); err != nil {
		t.Fatal(err)
	}
	file, err := os.OpenFile(original, os.O_RDWR, 0600)
	if err != nil {
		t.Fatal(err)
	}
	moved := filepath.Join(dir, "original-inode")
	if err := os.Rename(original, moved); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(original, []byte("substitute"), 0600); err != nil {
		t.Fatal(err)
	}
	req := bridgeRequest{
		operation:        OpReadFile,
		password:         append([]byte(nil), password...),
		repoPath:         repo,
		snapshotID:       second,
		archiveNodePath:  "retained/clip.m4a",
		archiveTarget:    file,
		archiveSizeBound: 64,
	}
	if err := pinRequestPaths(&req); err != nil {
		_ = file.Close()
		t.Fatal(err)
	}
	result, err := executeReadFile(context.Background(), req, func() {})
	closeRequestPaths(&req)
	if err != nil || result.status != StatusOk {
		t.Fatalf("substituted target download failed: %+v %v", result, err)
	}
	if !bytes.Equal(mustRead(t, moved), []byte("new-bytes")) || bytes.Equal(mustRead(t, original), []byte("new-bytes")) {
		t.Fatal("download followed the replaced path instead of the issued descriptor")
	}
}

func TestArchiveCancellationDoesNotPublish(t *testing.T) {
	repo, password := archiveTestRepository(t)
	source := filepath.Join(t.TempDir(), "stage")
	if err := os.Mkdir(source, 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(source, "clip.m4a"), []byte("kept"), 0600); err != nil {
		t.Fatal(err)
	}
	root, err := os.OpenRoot(source)
	if err != nil {
		t.Fatal(err)
	}
	req := bridgeRequest{
		operation:    OpArchiveSources,
		password:     append([]byte(nil), password...),
		repoPath:     repo,
		sourceName:   "retained",
		sourceRoot:   root,
		archiveNames: []string{"clip.m4a"},
	}
	if err := pinRequestPaths(&req); err != nil {
		_ = root.Close()
		t.Fatal(err)
	}
	testGate = func(stage string, _ context.Context) error {
		if stage == "mutation-entered" {
			return context.Canceled
		}
		return nil
	}
	t.Cleanup(func() { testGate = nil })
	result, err := executeArchiveSources(context.Background(), req, func() {})
	closeRequestPaths(&req)
	testGate = nil
	if err == nil || result.status == StatusOk || result.snapshotID != "" {
		t.Fatalf("cancelled archive published: %+v %v", result, err)
	}

	snapshot := archiveRetainedFile(t, repo, password, "retained", source, []string{"clip.m4a"})
	target := filepath.Join(t.TempDir(), "issued")
	if err := os.WriteFile(target, []byte("untouched"), 0600); err != nil {
		t.Fatal(err)
	}
	file, err := os.OpenFile(target, os.O_RDWR, 0600)
	if err != nil {
		t.Fatal(err)
	}
	download := bridgeRequest{
		operation:        OpReadFile,
		password:         append([]byte(nil), password...),
		repoPath:         repo,
		snapshotID:       snapshot,
		archiveNodePath:  "retained/clip.m4a",
		archiveTarget:    file,
		archiveSizeBound: 64,
	}
	if err := pinRequestPaths(&download); err != nil {
		_ = file.Close()
		t.Fatal(err)
	}
	testGate = func(stage string, _ context.Context) error {
		if stage == "download-entered" {
			return context.Canceled
		}
		return nil
	}
	cancelled, _ := executeReadFile(context.Background(), download, func() {})
	closeRequestPaths(&download)
	testGate = nil
	if cancelled.status == StatusOk || !bytes.Equal(mustRead(t, target), []byte("untouched")) {
		t.Fatalf("cancelled download published: %+v", cancelled)
	}

	file, err = os.OpenFile(target, os.O_RDWR, 0600)
	if err != nil {
		t.Fatal(err)
	}
	download.archiveTarget = file
	download.password = append([]byte(nil), password...)
	if err := pinRequestPaths(&download); err != nil {
		_ = file.Close()
		t.Fatal(err)
	}
	testGate = func(stage string, _ context.Context) error {
		if stage == "download-wrote" {
			return context.Canceled
		}
		return nil
	}
	uncertain, _ := executeReadFile(context.Background(), download, func() {})
	closeRequestPaths(&download)
	testGate = nil
	if uncertain.status == StatusOk || uncertain.status == StatusCancelled {
		t.Fatalf("write cancellation looked complete: %+v", uncertain)
	}
}
