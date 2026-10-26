// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

/*
#include <stdint.h>

#ifndef SOLSTONE_RESTIC_S3_REQUEST
#define SOLSTONE_RESTIC_S3_REQUEST
typedef struct {
    uint32_t version;
    int32_t operation;
    const char *endpoint; uint32_t endpoint_len;
    const char *region; uint32_t region_len;
    const char *bucket; uint32_t bucket_len;
    const char *prefix; uint32_t prefix_len;
    const char *access_key; uint32_t access_key_len;
    const char *secret_key; uint32_t secret_key_len;
    const char *session_token; uint32_t session_token_len;
    const char *password; uint32_t password_len;
    const char *recovery_password; uint32_t recovery_password_len;
    const char *state_path; uint32_t state_path_len;
    const char *source_path; uint32_t source_path_len;
    const char *restore_path; uint32_t restore_path_len;
    const char *snapshot_id; uint32_t snapshot_id_len;
    const char *workspace_path; uint32_t workspace_path_len;
} restic_s3_request;
#endif
*/
import "C"

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/restic/restic/internal/archiver"
	"github.com/restic/restic/internal/backend"
	"github.com/restic/restic/internal/data"
	"github.com/restic/restic/internal/repository"
	"github.com/restic/restic/internal/restic"
	"github.com/restic/restic/internal/ui/progress"
	"github.com/restic/restic/internal/walker"
	"golang.org/x/sys/unix"
)

const (
	maxArchiveNamesPayload = 1 << 20
	maxArchiveNames        = 4096
	maxNodeListingBytes    = 1 << 20
	archiveTag             = "spb-offload-archive"
)

type archiveNodeJSON struct {
	MessageType string `json:"message_type"`
	Type        string `json:"type"`
	Path        string `json:"path"`
	Name        string `json:"name"`
	Size        uint64 `json:"size"`
}

type archiveSummaryJSON struct {
	MessageType string `json:"message_type"`
	Complete    bool   `json:"complete"`
	NodeCount   uint64 `json:"node_count"`
}

func validLogicalRoot(root string) bool {
	return root != "" && root != "." && root != ".." && !strings.ContainsAny(root, `/\`) && utf8.ValidString(root) && !strings.ContainsRune(root, 0)
}

func validArchiveRelative(name string) bool {
	if name == "" || len(name) > maxPathLen || !utf8.ValidString(name) || strings.ContainsRune(name, 0) {
		return false
	}
	if strings.HasPrefix(name, "/") || strings.Contains(name, `\`) || strings.Contains(name, "//") {
		return false
	}
	for _, part := range strings.Split(name, "/") {
		if part == "" || part == "." || part == ".." {
			return false
		}
	}
	return true
}

func parseArchiveNames(payload []byte) ([]string, bool) {
	if len(payload) == 0 || len(payload) > maxArchiveNamesPayload {
		return nil, false
	}
	dec := json.NewDecoder(bytes.NewReader(payload))
	var names []string
	if err := dec.Decode(&names); err != nil || dec.More() {
		return nil, false
	}
	var trailing struct{}
	if err := dec.Decode(&trailing); !errors.Is(err, io.EOF) {
		return nil, false
	}
	if len(names) == 0 || len(names) > maxArchiveNames {
		return nil, false
	}
	seen := make(map[string]struct{}, len(names))
	for _, name := range names {
		if !validArchiveRelative(name) {
			return nil, false
		}
		if _, ok := seen[name]; ok {
			return nil, false
		}
		seen[name] = struct{}{}
	}
	return names, true
}

func archiveSelection(root string, names []string) func(string) bool {
	allowed := map[string]struct{}{filepath.Clean(root): {}}
	for _, name := range names {
		full := filepath.Clean(filepath.Join(root, filepath.FromSlash(name)))
		for current := full; ; current = filepath.Dir(current) {
			allowed[current] = struct{}{}
			if current == filepath.Clean(root) || current == "." || current == string(filepath.Separator) {
				break
			}
		}
	}
	return func(item string) bool {
		_, ok := allowed[filepath.Clean(item)]
		return ok
	}
}

func rejectArchiveSelection(req bridgeRequest) (bridgeResult, bool) {
	if !validLogicalRoot(req.sourceName) || len(req.archiveNames) == 0 || len(req.archiveNames) > maxArchiveNames || req.sourceRoot == nil {
		return bridgeResult{status: StatusInvalidArgument, diagnostic: DiagInvalidArgument}, true
	}
	seen := make(map[string]struct{}, len(req.archiveNames))
	for _, name := range req.archiveNames {
		if !validArchiveRelative(name) {
			return bridgeResult{status: StatusInvalidArgument, diagnostic: DiagInvalidArgument}, true
		}
		if _, ok := seen[name]; ok {
			return bridgeResult{status: StatusInvalidArgument, diagnostic: DiagInvalidArgument}, true
		}
		seen[name] = struct{}{}
		info, err := req.sourceRoot.Lstat(name)
		if err != nil || info == nil || !info.Mode().IsRegular() {
			return bridgeResult{status: StatusInvalidArgument, diagnostic: DiagInvalidArgument}, true
		}
	}
	return bridgeResult{}, false
}

func encodeNodeListing(nodes []archiveNodeJSON) ([]byte, int32, string) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	for _, node := range nodes {
		if node.MessageType != "node" || node.Type == "" || node.Path == "" || node.Name == "" {
			return nil, StatusFailed, DiagIncompleteDiscovery
		}
		if err := enc.Encode(node); err != nil || buf.Len() > maxNodeListingBytes {
			if buf.Len() > maxNodeListingBytes {
				return nil, StatusCapExceeded, DiagCapExceeded
			}
			return nil, StatusFailed, DiagIncompleteDiscovery
		}
	}
	summary := archiveSummaryJSON{MessageType: "summary", Complete: true, NodeCount: uint64(len(nodes))}
	if err := enc.Encode(summary); err != nil {
		return nil, StatusFailed, DiagIncompleteDiscovery
	}
	if buf.Len() == 0 || buf.Len() > maxNodeListingBytes {
		if buf.Len() > maxNodeListingBytes {
			return nil, StatusCapExceeded, DiagCapExceeded
		}
		return nil, StatusFailed, DiagIncompleteDiscovery
	}
	return buf.Bytes(), StatusOk, ""
}

func finishNodeListing(nodes []archiveNodeJSON, walkErr error) bridgeResult {
	if walkErr != nil {
		return bridgeResult{status: StatusFailed, diagnostic: DiagIncompleteDiscovery}
	}
	meta, status, diag := encodeNodeListing(nodes)
	if status != StatusOk {
		return bridgeResult{status: status, diagnostic: diag}
	}
	return bridgeResult{status: StatusOk, metadata: meta, metadataStatus: StatusOk}
}

func openArchiveRepository(ctx context.Context, req bridgeRequest, exclusive bool) (*repository.Repository, backend.Backend, func(), context.Context, bridgeResult) {
	failed := func(result bridgeResult) (*repository.Repository, backend.Backend, func(), context.Context, bridgeResult) {
		return nil, nil, nil, ctx, result
	}
	if ctx.Err() != nil {
		return failed(bridgeResult{status: StatusCancelled, diagnostic: DiagCancelled})
	}
	pw, clearPw := passwordString(req.password)
	defer clearPw()
	be, err := operationBackend(ctx, req, false)
	if err != nil {
		return failed(bridgeResult{status: StatusFailed, diagnostic: DiagRepoMissing})
	}
	closeBackend := func() {
		_ = be.Close()
	}
	repo, err := repository.New(guardedBackend{be}, repository.Options{})
	if err != nil {
		closeBackend()
		return failed(bridgeResult{status: StatusFailed, diagnostic: DiagInternal})
	}
	if _, err = safeScanKey(ctx, repo, pw); err != nil {
		if ctx.Err() != nil {
			closeBackend()
			return failed(bridgeResult{status: StatusCancelled, diagnostic: DiagCancelled})
		}
		result := keyRefusal(ctx, be, err)
		closeBackend()
		return failed(result)
	}
	unlock, lockCtx, err := lockRepoForRequest(ctx, req, repo, exclusive)
	if err != nil {
		closeBackend()
		return failed(bridgeResult{status: StatusFailed, diagnostic: DiagLocked})
	}
	if err = repo.LoadIndex(lockCtx, &progress.NoopPrinter{}); err != nil {
		unlock()
		closeBackend()
		return failed(refusalResult(err, DiagRepoCorrupt))
	}
	return repo, be, unlock, lockCtx, bridgeResult{status: StatusOk}
}

func releaseArchiveRepository(be backend.Backend, unlock func()) {
	if unlock != nil {
		unlock()
	}
	if be != nil {
		_ = be.Close()
	}
}

func executeArchiveSources(ctx context.Context, req bridgeRequest, setWrote func()) (bridgeResult, error) {
	if !req.pathsPinned {
		if err := pinRequestPaths(&req); err != nil {
			return mapPinError(req.operation, err), err
		}
		defer closeRequestPaths(&req)
	}
	if rejected, refuse := rejectArchiveSelection(req); refuse {
		return rejected, errors.New("invalid archive selection")
	}
	repo, be, unlock, lockCtx, opened := openArchiveRepository(ctx, req, true)
	if opened.status != StatusOk {
		return opened, errors.New(opened.diagnostic)
	}
	defer releaseArchiveRepository(be, unlock)

	markWrote := func() {
		if setWrote != nil {
			setWrote()
		}
	}
	markWrote()
	if testGate != nil {
		if err := testGate("mutation-entered", lockCtx); err != nil {
			return bridgeResult{status: StatusCancelled, diagnostic: DiagCancelled}, err
		}
	}
	if lockCtx.Err() != nil {
		return bridgeResult{status: StatusCancelled, diagnostic: DiagCancelled}, lockCtx.Err()
	}

	rootFS := newRootFS(req.sourceRoot, req.sourceName, nil)
	rootFS.owned = req.owned
	arch := archiver.New(repo, rootFS, archiver.Options{})
	arch.SelectByName = archiveSelection(req.sourceName, req.archiveNames)
	arch.Error = func(_ string, err error) error { return err }
	opts := archiver.SnapshotOptions{
		Tags:     data.TagList{archiveTag},
		Hostname: "solstone",
		Time:     time.Now(),
	}
	_, snapshotID, _, err := arch.Snapshot(lockCtx, []string{req.sourceName}, opts)
	if err != nil {
		if strings.Contains(err.Error(), "snapshot is empty") {
			return bridgeResult{status: StatusFailed, diagnostic: DiagSnapshotEmpty}, err
		}
		if lockCtx.Err() != nil {
			return bridgeResult{status: StatusCancelledUncertain, diagnostic: DiagCancelledUncertain}, lockCtx.Err()
		}
		return bridgeResult{status: StatusFailed, diagnostic: scrubError(err, req)}, err
	}
	if !validS3ID(snapshotID.String()) {
		return bridgeResult{status: StatusFailed, diagnostic: DiagInternal}, errors.New("archive snapshot id is not a full id")
	}
	return bridgeResult{status: StatusOk, snapshotID: snapshotID.String()}, nil
}

func collectArchiveNodes(ctx context.Context, repo *repository.Repository, tree restic.ID) ([]archiveNodeJSON, error) {
	var nodes []archiveNodeJSON
	err := walker.Walk(ctx, repo, tree, walker.WalkVisitor{
		ProcessNode: func(_ restic.ID, nodePath string, node *data.Node, nodeErr error) error {
			if nodeErr != nil {
				return nodeErr
			}
			if node == nil {
				return nil
			}
			recordPath := strings.TrimPrefix(nodePath, "/")
			recordType := string(node.Type)
			if recordPath == "" || node.Name == "" || recordType == "" {
				return errors.New("archive node is incomplete")
			}
			nodes = append(nodes, archiveNodeJSON{
				MessageType: "node",
				Type:        recordType,
				Path:        recordPath,
				Name:        node.Name,
				Size:        node.Size,
			})
			return nil
		},
	})
	if err != nil {
		return nil, err
	}
	return nodes, nil
}

func loadArchiveTree(ctx context.Context, repo *repository.Repository, snapshotID string) (restic.ID, bridgeResult) {
	if !validS3ID(snapshotID) {
		return restic.ID{}, bridgeResult{status: StatusInvalidArgument, diagnostic: DiagInvalidArgument}
	}
	id, err := restic.ParseID(snapshotID)
	if err != nil {
		return restic.ID{}, bridgeResult{status: StatusInvalidArgument, diagnostic: DiagInvalidArgument}
	}
	snapshot, err := data.LoadSnapshot(ctx, repo, id)
	if err != nil || snapshot == nil || snapshot.Tree == nil {
		return restic.ID{}, bridgeResult{status: StatusFailed, diagnostic: DiagRepoCorrupt}
	}
	return *snapshot.Tree, bridgeResult{status: StatusOk}
}

func executeListNodes(ctx context.Context, req bridgeRequest) (bridgeResult, error) {
	if !req.pathsPinned {
		if err := pinRequestPaths(&req); err != nil {
			return mapPinError(req.operation, err), err
		}
		defer closeRequestPaths(&req)
	}
	repo, be, unlock, lockCtx, opened := openArchiveRepository(ctx, req, false)
	if opened.status != StatusOk {
		return opened, errors.New(opened.diagnostic)
	}
	defer releaseArchiveRepository(be, unlock)
	tree, loaded := loadArchiveTree(lockCtx, repo, req.snapshotID)
	if loaded.status != StatusOk {
		return loaded, errors.New(loaded.diagnostic)
	}
	nodes, err := collectArchiveNodes(lockCtx, repo, tree)
	result := finishNodeListing(nodes, err)
	if result.status == StatusOk {
		result.snapshotID = req.snapshotID
	}
	if result.status != StatusOk {
		return result, errors.New(result.diagnostic)
	}
	return result, nil
}

type archiveFileBody struct {
	size    uint64
	content restic.IDs
}

func findArchiveFile(ctx context.Context, repo *repository.Repository, tree restic.ID, want string) (archiveFileBody, bool, error) {
	var body archiveFileBody
	found := false
	err := walker.Walk(ctx, repo, tree, walker.WalkVisitor{
		ProcessNode: func(_ restic.ID, nodePath string, node *data.Node, nodeErr error) error {
			if nodeErr != nil {
				return nodeErr
			}
			if node == nil {
				return nil
			}
			recordPath := strings.TrimPrefix(nodePath, "/")
			if recordPath == "" || node.Name == "" || string(node.Type) == "" {
				return errors.New("archive node is incomplete")
			}
			if recordPath != want || node.Type != data.NodeTypeFile {
				return nil
			}
			if found {
				return errors.New("archive file node is duplicated")
			}
			found = true
			body.size = node.Size
			body.content = append(restic.IDs(nil), node.Content...)
			return nil
		},
	})
	if err != nil {
		return archiveFileBody{}, false, err
	}
	return body, found, nil
}

func executeReadFile(ctx context.Context, req bridgeRequest, setWrote func()) (bridgeResult, error) {
	if !req.pathsPinned {
		if err := pinRequestPaths(&req); err != nil {
			return mapPinError(req.operation, err), err
		}
		defer closeRequestPaths(&req)
	}
	if req.archiveTarget == nil || !validArchiveRelative(req.archiveNodePath) || !validS3ID(req.snapshotID) {
		return bridgeResult{status: StatusInvalidArgument, diagnostic: DiagInvalidArgument}, errors.New("invalid archive download")
	}
	repo, be, unlock, lockCtx, opened := openArchiveRepository(ctx, req, false)
	if opened.status != StatusOk {
		return opened, errors.New(opened.diagnostic)
	}
	defer releaseArchiveRepository(be, unlock)
	tree, loaded := loadArchiveTree(lockCtx, repo, req.snapshotID)
	if loaded.status != StatusOk {
		return loaded, errors.New(loaded.diagnostic)
	}
	if testGate != nil {
		if err := testGate("download-entered", lockCtx); err != nil {
			return bridgeResult{status: StatusCancelled, diagnostic: DiagCancelled}, err
		}
	}
	if lockCtx.Err() != nil {
		return bridgeResult{status: StatusCancelled, diagnostic: DiagCancelled}, lockCtx.Err()
	}
	body, found, err := findArchiveFile(lockCtx, repo, tree, req.archiveNodePath)
	if err != nil {
		return bridgeResult{status: StatusFailed, diagnostic: DiagIncompleteDiscovery}, err
	}
	if !found {
		return bridgeResult{status: StatusFailed, diagnostic: DiagSnapshotNodeUnavailable}, errors.New("archive file is absent")
	}
	if body.size > req.archiveSizeBound || (body.size == 0 && len(body.content) != 0) {
		status := StatusCapExceeded
		diag := DiagCapExceeded
		if body.size == 0 && len(body.content) != 0 {
			status = StatusFailed
			diag = DiagIncompleteDiscovery
		}
		return bridgeResult{status: status, diagnostic: diag}, errors.New(diag)
	}
	wrote := false
	markWrote := func() {
		if !wrote && setWrote != nil {
			setWrote()
		}
		wrote = true
	}
	var drained uint64
	for _, id := range body.content {
		if lockCtx.Err() != nil {
			if wrote {
				return bridgeResult{status: StatusCancelledUncertain, diagnostic: DiagCancelledUncertain}, lockCtx.Err()
			}
			return bridgeResult{status: StatusCancelled, diagnostic: DiagCancelled}, lockCtx.Err()
		}
		blob, loadErr := repo.LoadBlob(lockCtx, restic.DataBlob, id, nil)
		if loadErr != nil || (drained+uint64(len(blob)) < drained) || drained+uint64(len(blob)) > body.size {
			if wrote || len(blob) > 0 && loadErr == nil {
				return bridgeResult{status: StatusFailed, diagnostic: DiagIoFailed}, errors.New(DiagIoFailed)
			}
			return bridgeResult{status: StatusFailed, diagnostic: DiagIncompleteDiscovery}, errors.New(DiagIncompleteDiscovery)
		}
		if len(blob) == 0 {
			continue
		}
		written, writeErr := req.archiveTarget.Write(blob)
		if written > 0 {
			markWrote()
			drained += uint64(written)
		}
		if writeErr != nil || written != len(blob) {
			if wrote {
				if lockCtx.Err() != nil {
					return bridgeResult{status: StatusCancelledUncertain, diagnostic: DiagCancelledUncertain}, lockCtx.Err()
				}
				return bridgeResult{status: StatusFailed, diagnostic: DiagIoFailed}, errors.New(DiagIoFailed)
			}
			return bridgeResult{status: StatusFailed, diagnostic: DiagIoFailed}, errors.New(DiagIoFailed)
		}
		if testGate != nil {
			if err := testGate("download-wrote", lockCtx); err != nil {
				return bridgeResult{status: StatusCancelledUncertain, diagnostic: DiagCancelledUncertain}, err
			}
		}
	}
	if drained != body.size || drained > req.archiveSizeBound {
		if wrote {
			return bridgeResult{status: StatusFailed, diagnostic: DiagIoFailed}, errors.New(DiagIoFailed)
		}
		return bridgeResult{status: StatusFailed, diagnostic: DiagIncompleteDiscovery}, errors.New(DiagIncompleteDiscovery)
	}
	meta, err := json.Marshal(struct {
		Drained uint64 `json:"drained"`
	}{Drained: drained})
	if err != nil || len(meta) == 0 || len(meta) > maxResultMetadata {
		return bridgeResult{status: StatusFailed, diagnostic: DiagIncompleteDiscovery}, errors.New(DiagIncompleteDiscovery)
	}
	return bridgeResult{status: StatusOk, snapshotID: req.snapshotID, metadata: meta, metadataStatus: StatusOk}, nil
}

func dupOwnedFD(fd int) (*os.File, int32) {
	if fd < 0 {
		return nil, StatusInvalidArgument
	}
	dup, err := unix.FcntlInt(uintptr(fd), unix.F_DUPFD_CLOEXEC, 0)
	if err != nil {
		return nil, StatusInvalidArgument
	}
	file := os.NewFile(uintptr(dup), "archive-fd")
	if file == nil {
		_ = unix.Close(dup)
		return nil, StatusInvalidArgument
	}
	return file, StatusOk
}

func admitArchiveDirectory(fd int, entry *ownedCall) (*os.Root, int32) {
	expected, status := readBorrowedDirectoryIdentity(fd)
	if status != StatusOk {
		return nil, status
	}
	file, status := dupOwnedFD(fd)
	if status != StatusOk {
		return nil, status
	}
	if !matchesBorrowedDirectory("archive-source", file, expected) {
		_ = file.Close()
		return nil, StatusInvalidArgument
	}
	root, err := adoptOwnedRoot(fmt.Sprintf("/dev/fd/%d", int(file.Fd())), file, entry)
	closeErr := file.Close()
	if err != nil || closeErr != nil {
		if root != nil {
			_ = root.Close()
		}
		return nil, StatusInvalidArgument
	}
	return root, StatusOk
}

func admitArchiveTarget(fd int) (*os.File, int32) {
	if fd < 0 {
		return nil, StatusInvalidArgument
	}
	var before unix.Stat_t
	if unix.Fstat(fd, &before) != nil || before.Mode&unix.S_IFMT != unix.S_IFREG {
		return nil, StatusInvalidArgument
	}
	file, status := dupOwnedFD(fd)
	if status != StatusOk {
		return nil, status
	}
	var after unix.Stat_t
	if unix.Fstat(int(file.Fd()), &after) != nil || after.Mode&unix.S_IFMT != unix.S_IFREG || after.Dev != before.Dev || after.Ino != before.Ino {
		_ = file.Close()
		return nil, StatusInvalidArgument
	}
	return file, StatusOk
}

func archiveBytes(ptr *C.char, length C.uint32_t, limit uint32) ([]byte, int32) {
	if uint32(length) > limit || (length > 0 && ptr == nil) {
		if uint32(length) > limit {
			return nil, StatusCapExceeded
		}
		return nil, StatusInvalidArgument
	}
	if length == 0 {
		return nil, StatusOk
	}
	return copyCBytes(ptr, length), StatusOk
}

func archiveText(ptr *C.char, length C.uint32_t, limit uint32, required bool) (string, int32) {
	buf, status := archiveBytes(ptr, length, limit)
	if status != StatusOk {
		return "", status
	}
	if required && len(buf) == 0 {
		return "", StatusInvalidArgument
	}
	text := string(buf)
	zeroBytes(buf)
	if !utf8.ValidString(text) {
		return "", StatusInvalidArgument
	}
	return text, StatusOk
}

func reserveArchiveCall(outID *C.uint64_t) (*ownedCall, C.int32_t) {
	if outID == nil {
		return nil, C.int32_t(StatusInvalidArgument)
	}
	*outID = 0
	entry, status := reserveOwnedCall()
	if status != StatusOk {
		return nil, C.int32_t(status)
	}
	return entry, C.int32_t(StatusOk)
}

func publishArchiveCall(entry *ownedCall, req bridgeRequest, outID *C.uint64_t) C.int32_t {
	entry.retain(&req)
	id, status := startBridgeCall(req)
	if status != StatusOk {
		return C.int32_t(status)
	}
	*outID = C.uint64_t(id)
	return C.int32_t(StatusOk)
}

//export restic_bridge_archive_sources_v1_start
func restic_bridge_archive_sources_v1_start(
	version C.uint32_t,
	password *C.char, passwordLen C.uint32_t,
	repoPath *C.char, repoPathLen C.uint32_t,
	logicalRoot *C.char, logicalRootLen C.uint32_t,
	sourceFD C.int,
	namesJSON *C.char, namesLen C.uint32_t,
	outID *C.uint64_t,
) (returned C.int32_t) {
	if version != 1 {
		return C.int32_t(StatusInvalidArgument)
	}
	names, status := archiveBytes(namesJSON, namesLen, maxArchiveNamesPayload)
	if status != StatusOk {
		return C.int32_t(status)
	}
	parsed, ok := parseArchiveNames(names)
	zeroBytes(names)
	if !ok {
		return C.int32_t(StatusInvalidArgument)
	}
	rootName, status := archiveText(logicalRoot, logicalRootLen, maxPathLen, true)
	if status != StatusOk || !validLogicalRoot(rootName) {
		return C.int32_t(StatusInvalidArgument)
	}
	repo, status := archiveText(repoPath, repoPathLen, maxPathLen, true)
	if status != StatusOk || sanitizePath(repo) != nil {
		return C.int32_t(StatusInvalidArgument)
	}
	secret, status := archiveBytes(password, passwordLen, maxPasswordLen)
	if status != StatusOk || len(secret) == 0 {
		zeroBytes(secret)
		return C.int32_t(StatusInvalidArgument)
	}
	entry, returned := reserveArchiveCall(outID)
	if entry == nil {
		zeroBytes(secret)
		return returned
	}
	defer func() { returned = C.int32_t(entry.finishAdmission(int32(returned))) }()
	req := bridgeRequest{
		owned:        entry,
		operation:    OpArchiveSources,
		password:     secret,
		repoPath:     repo,
		sourceName:   rootName,
		archiveNames: parsed,
	}
	entry.retain(&req)
	root, status := admitArchiveDirectory(int(sourceFD), entry)
	if status != StatusOk {
		return C.int32_t(status)
	}
	req.sourceRoot = root
	if err := pinRequestPaths(&req); err != nil {
		return C.int32_t(StatusInvalidArgument)
	}
	return publishArchiveCall(entry, req, outID)
}

//export restic_bridge_list_nodes_v1_start
func restic_bridge_list_nodes_v1_start(
	version C.uint32_t,
	password *C.char, passwordLen C.uint32_t,
	repoPath *C.char, repoPathLen C.uint32_t,
	snapshotID *C.char, snapshotLen C.uint32_t,
	outID *C.uint64_t,
) (returned C.int32_t) {
	if version != 1 {
		return C.int32_t(StatusInvalidArgument)
	}
	repo, status := archiveText(repoPath, repoPathLen, maxPathLen, true)
	if status != StatusOk || sanitizePath(repo) != nil {
		return C.int32_t(StatusInvalidArgument)
	}
	snapshot, status := archiveText(snapshotID, snapshotLen, idHexLen, true)
	if status != StatusOk || !validS3ID(snapshot) {
		return C.int32_t(StatusInvalidArgument)
	}
	secret, status := archiveBytes(password, passwordLen, maxPasswordLen)
	if status != StatusOk || len(secret) == 0 {
		zeroBytes(secret)
		return C.int32_t(StatusInvalidArgument)
	}
	entry, returned := reserveArchiveCall(outID)
	if entry == nil {
		zeroBytes(secret)
		return returned
	}
	defer func() { returned = C.int32_t(entry.finishAdmission(int32(returned))) }()
	req := bridgeRequest{
		owned:      entry,
		operation:  OpListNodes,
		password:   secret,
		repoPath:   repo,
		snapshotID: snapshot,
	}
	entry.retain(&req)
	if err := pinRequestPaths(&req); err != nil {
		return C.int32_t(StatusInvalidArgument)
	}
	return publishArchiveCall(entry, req, outID)
}

//export restic_bridge_download_exact_v1_start
func restic_bridge_download_exact_v1_start(
	version C.uint32_t,
	password *C.char, passwordLen C.uint32_t,
	repoPath *C.char, repoPathLen C.uint32_t,
	snapshotID *C.char, snapshotLen C.uint32_t,
	nodePath *C.char, nodePathLen C.uint32_t,
	targetFD C.int,
	sizeBound C.uint64_t,
	outID *C.uint64_t,
) (returned C.int32_t) {
	if version != 1 {
		return C.int32_t(StatusInvalidArgument)
	}
	repo, status := archiveText(repoPath, repoPathLen, maxPathLen, true)
	if status != StatusOk || sanitizePath(repo) != nil {
		return C.int32_t(StatusInvalidArgument)
	}
	snapshot, status := archiveText(snapshotID, snapshotLen, idHexLen, true)
	if status != StatusOk || !validS3ID(snapshot) {
		return C.int32_t(StatusInvalidArgument)
	}
	node, status := archiveText(nodePath, nodePathLen, maxPathLen, true)
	if status != StatusOk || !validArchiveRelative(node) {
		return C.int32_t(StatusInvalidArgument)
	}
	secret, status := archiveBytes(password, passwordLen, maxPasswordLen)
	if status != StatusOk || len(secret) == 0 {
		zeroBytes(secret)
		return C.int32_t(StatusInvalidArgument)
	}
	entry, returned := reserveArchiveCall(outID)
	if entry == nil {
		zeroBytes(secret)
		return returned
	}
	defer func() { returned = C.int32_t(entry.finishAdmission(int32(returned))) }()
	target, status := admitArchiveTarget(int(targetFD))
	if status != StatusOk {
		return C.int32_t(status)
	}
	req := bridgeRequest{
		owned:            entry,
		operation:        OpReadFile,
		password:         secret,
		repoPath:         repo,
		snapshotID:       snapshot,
		archiveNodePath:  node,
		archiveTarget:    target,
		archiveSizeBound: uint64(sizeBound),
	}
	entry.retain(&req)
	if err := pinRequestPaths(&req); err != nil {
		return C.int32_t(StatusInvalidArgument)
	}
	return publishArchiveCall(entry, req, outID)
}

func admitArchiveS3(request *C.restic_s3_request, stateFD C.int, operation int32, snapshotRequired bool, entry *ownedCall) (bridgeRequest, int32) {
	if request == nil || request.version != 1 || int32(request.operation) != operation {
		return bridgeRequest{}, StatusInvalidArgument
	}
	if request.source_path_len != 0 || request.restore_path_len != 0 || request.workspace_path_len != 0 || request.recovery_password_len != 0 {
		return bridgeRequest{}, StatusInvalidArgument
	}
	if snapshotRequired {
		if request.snapshot_id == nil || request.snapshot_id_len != idHexLen {
			return bridgeRequest{}, StatusInvalidArgument
		}
	} else if request.snapshot_id_len != 0 {
		return bridgeRequest{}, StatusInvalidArgument
	}
	fields := [14]requestField{
		{uint32(request.endpoint_len), request.endpoint != nil},
		{uint32(request.region_len), request.region != nil},
		{uint32(request.bucket_len), request.bucket != nil},
		{uint32(request.prefix_len), request.prefix != nil},
		{uint32(request.access_key_len), request.access_key != nil},
		{uint32(request.secret_key_len), request.secret_key != nil},
		{uint32(request.session_token_len), request.session_token != nil},
		{uint32(request.password_len), request.password != nil},
		{uint32(request.recovery_password_len), request.recovery_password != nil},
		{uint32(request.state_path_len), request.state_path != nil},
		{uint32(request.source_path_len), request.source_path != nil},
		{uint32(request.restore_path_len), request.restore_path != nil},
		{uint32(request.snapshot_id_len), request.snapshot_id != nil},
		{uint32(request.workspace_path_len), request.workspace_path != nil},
	}
	caps := [14]uint32{maxHTTPURLBytes, 64, 63, 512, 256, 1024, 16384, maxPasswordLen, maxPasswordLen, maxPathLen, maxPathLen, maxPathLen, idHexLen, maxPathLen}
	for index, field := range fields {
		if field.length > caps[index] {
			return bridgeRequest{}, StatusCapExceeded
		}
		if field.length != 0 && !field.present {
			return bridgeRequest{}, StatusInvalidArgument
		}
	}
	for _, index := range []int{0, 1, 2, 3, 4, 5, 7, 9} {
		if fields[index].length == 0 {
			return bridgeRequest{}, StatusInvalidArgument
		}
	}
	readString := func(pointer *C.char, length C.uint32_t) string {
		owned := copyCBytes(pointer, length)
		defer zeroBytes(owned)
		return string(owned)
	}
	cfg := s3Config{
		endpoint: readString(request.endpoint, request.endpoint_len),
		region:   readString(request.region, request.region_len),
		bucket:   readString(request.bucket, request.bucket_len),
		prefix:   readString(request.prefix, request.prefix_len),
		access:   readString(request.access_key, request.access_key_len),
		secret:   readString(request.secret_key, request.secret_key_len),
		token:    readString(request.session_token, request.session_token_len),
	}
	if cfg.validate() != nil {
		return bridgeRequest{}, StatusInvalidArgument
	}
	snapshot := ""
	if snapshotRequired {
		snapshot = readString(request.snapshot_id, request.snapshot_id_len)
		if !validS3ID(snapshot) {
			return bridgeRequest{}, StatusInvalidArgument
		}
	}
	stateIdentity, status := readBorrowedDirectoryIdentity(int(stateFD))
	if status != StatusOk {
		return bridgeRequest{}, status
	}
	statePath := readString(request.state_path, request.state_path_len)
	owner, err := newS3OwnerExpected(statePath, cfg, entry, stateIdentity)
	if err != nil {
		if errors.Is(err, errS3OwnerBusy) {
			return bridgeRequest{}, StatusBusy
		}
		if errors.Is(err, errS3OwnerIO) {
			return bridgeRequest{}, StatusFailed
		}
		return bridgeRequest{}, StatusInvalidArgument
	}
	req := bridgeRequest{
		owned:      entry,
		operation:  operation,
		password:   copyCBytes(request.password, request.password_len),
		snapshotID: snapshot,
		remote:     &s3RemoteRequest{cfg: cfg, owner: owner},
	}
	entry.retain(&req)
	if err := pinDataPaths(&req, []*os.File{owner.directory}); err != nil {
		return req, StatusInvalidArgument
	}
	return req, StatusOk
}

func applyArchiveAppendOnly(req *bridgeRequest, appendOnly C.uint32_t) int32 {
	if appendOnly != 0 && appendOnly != 1 {
		return StatusInvalidArgument
	}
	if appendOnly == 1 && req.remote != nil {
		req.remote.cfg.appendOnly = true
	}
	return StatusOk
}

//export restic_bridge_s3_archive_sources_v1_start
func restic_bridge_s3_archive_sources_v1_start(
	request *C.restic_s3_request,
	stateFD C.int,
	sourceFD C.int,
	logicalRoot *C.char, logicalRootLen C.uint32_t,
	namesJSON *C.char, namesLen C.uint32_t,
	appendOnly C.uint32_t,
	outID *C.uint64_t,
) (returned C.int32_t) {
	names, status := archiveBytes(namesJSON, namesLen, maxArchiveNamesPayload)
	if status != StatusOk {
		return C.int32_t(status)
	}
	parsed, ok := parseArchiveNames(names)
	zeroBytes(names)
	if !ok {
		return C.int32_t(StatusInvalidArgument)
	}
	rootName, status := archiveText(logicalRoot, logicalRootLen, maxPathLen, true)
	if status != StatusOk || !validLogicalRoot(rootName) {
		return C.int32_t(StatusInvalidArgument)
	}
	entry, returned := reserveArchiveCall(outID)
	if entry == nil {
		return returned
	}
	defer func() { returned = C.int32_t(entry.finishAdmission(int32(returned))) }()
	req, status := admitArchiveS3(request, stateFD, OpArchiveSources, false, entry)
	if status != StatusOk {
		return C.int32_t(status)
	}
	if status = applyArchiveAppendOnly(&req, appendOnly); status != StatusOk {
		return C.int32_t(status)
	}
	root, status := admitArchiveDirectory(int(sourceFD), entry)
	if status != StatusOk {
		return C.int32_t(status)
	}
	req.sourceRoot = root
	req.sourceName = rootName
	req.archiveNames = parsed
	return publishArchiveCall(entry, req, outID)
}

//export restic_bridge_s3_list_nodes_v1_start
func restic_bridge_s3_list_nodes_v1_start(
	request *C.restic_s3_request,
	stateFD C.int,
	appendOnly C.uint32_t,
	outID *C.uint64_t,
) (returned C.int32_t) {
	entry, returned := reserveArchiveCall(outID)
	if entry == nil {
		return returned
	}
	defer func() { returned = C.int32_t(entry.finishAdmission(int32(returned))) }()
	req, status := admitArchiveS3(request, stateFD, OpListNodes, true, entry)
	if status != StatusOk {
		return C.int32_t(status)
	}
	if status = applyArchiveAppendOnly(&req, appendOnly); status != StatusOk {
		return C.int32_t(status)
	}
	return publishArchiveCall(entry, req, outID)
}

//export restic_bridge_s3_download_exact_v1_start
func restic_bridge_s3_download_exact_v1_start(
	request *C.restic_s3_request,
	stateFD C.int,
	targetFD C.int,
	nodePath *C.char, nodePathLen C.uint32_t,
	sizeBound C.uint64_t,
	appendOnly C.uint32_t,
	outID *C.uint64_t,
) (returned C.int32_t) {
	node, status := archiveText(nodePath, nodePathLen, maxPathLen, true)
	if status != StatusOk || !validArchiveRelative(node) {
		return C.int32_t(StatusInvalidArgument)
	}
	entry, returned := reserveArchiveCall(outID)
	if entry == nil {
		return returned
	}
	defer func() { returned = C.int32_t(entry.finishAdmission(int32(returned))) }()
	target, status := admitArchiveTarget(int(targetFD))
	if status != StatusOk {
		return C.int32_t(status)
	}
	req, status := admitArchiveS3(request, stateFD, OpReadFile, true, entry)
	if status != StatusOk {
		_ = target.Close()
		return C.int32_t(status)
	}
	if status = applyArchiveAppendOnly(&req, appendOnly); status != StatusOk {
		_ = target.Close()
		return C.int32_t(status)
	}
	req.archiveNodePath = node
	req.archiveTarget = target
	req.archiveSizeBound = uint64(sizeBound)
	return publishArchiveCall(entry, req, outID)
}
