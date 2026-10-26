//go:build integration

// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/restic/restic/internal/data"
	"github.com/restic/restic/internal/repository"
	"github.com/restic/restic/internal/restic"
)

type retainedSnapshot struct {
	id    string
	files map[string][]byte
}

func TestIntegrationRetentionIndexCapRefusesBeforeMutation(t *testing.T) {
	repo, source, password, _ := retentionRepository(t, false)
	for i := 0; i < 3; i++ {
		replaceSource(t, source, map[string][]byte{"extra.txt": []byte(fmt.Sprintf("extra-%d", i))})
		result, err := executeSnapshot(context.Background(), bridgeRequest{
			operation: OpSnapshot, password: []byte(password), repoPath: repo, sourcePath: source,
		}, func() {})
		if err != nil || result.status != StatusOk {
			t.Fatalf("extra snapshot: %+v %v", result, err)
		}
	}
	indexes, err := os.ReadDir(filepath.Join(repo, "index"))
	if err != nil || len(indexes) != maxIndexListCount+1 {
		t.Fatalf("cap fixture: indexes=%d error=%v", len(indexes), err)
	}
	beforeIDs, beforePacks := snapshotIDSet(t, repo, password), packNames(t, repo)
	wrote := false
	result, err := executeRetention(context.Background(), bridgeRequest{
		operation: OpRetention, password: []byte(password), repoPath: repo,
		retentionPolicy: &retentionPolicy{hourly: 1, tags: []data.TagList{{archiveKeepTag}, {offloadKeepTag}}},
	}, func() { wrote = true })
	if !errors.Is(err, errResourceLimit) || result.status != StatusCapExceeded || result.diagnostic != DiagCapExceeded || wrote {
		t.Fatalf("cap changed repository: result=%+v error=%v wrote=%v", result, err, wrote)
	}
	if !sameIDSet(snapshotIDSet(t, repo, password), beforeIDs) || !sameNameSet(packNames(t, repo), beforePacks) {
		t.Fatal("index cap changed snapshots or packs")
	}
}

func TestIntegrationRetentionPostPlanIndexDriftRefusesBeforeMutation(t *testing.T) {
	repo, _, password, _ := retentionRepository(t, false)
	beforeIDs, beforePacks := snapshotIDSet(t, repo, password), packNames(t, repo)
	indexes, err := os.ReadDir(filepath.Join(repo, "index"))
	if err != nil || len(indexes) == 0 {
		t.Fatalf("index fixture: %v", err)
	}
	wrote := false
	testGate = func(stage string, _ context.Context) error {
		if stage == "retention-before-mutation" {
			body, err := os.ReadFile(filepath.Join(repo, "index", indexes[0].Name()))
			if err != nil {
				return err
			}
			return os.WriteFile(filepath.Join(repo, "index", "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"), body, 0600)
		}
		return nil
	}
	t.Cleanup(func() { testGate = nil })
	result, err := executeRetention(context.Background(), bridgeRequest{
		operation: OpRetention, password: []byte(password), repoPath: repo,
		retentionPolicy: &retentionPolicy{hourly: 1, tags: []data.TagList{{archiveKeepTag}, {offloadKeepTag}}},
	}, func() { wrote = true })
	testGate = nil
	if !errors.Is(err, errRetentionChanged) || result.status != StatusFailed || wrote {
		t.Fatalf("post-plan drift changed repository: result=%+v error=%v wrote=%v", result, err, wrote)
	}
	if !sameIDSet(snapshotIDSet(t, repo, password), beforeIDs) || !sameNameSet(packNames(t, repo), beforePacks) {
		t.Fatal("post-plan index drift deleted snapshots or packs")
	}
}

func TestIntegrationRetentionSelectionPruneAndRestore(t *testing.T) {
	repo, source, password, made := retentionRepository(t, true)
	ids := snapshotIDSet(t, repo, password)
	if len(ids) != len(made) {
		t.Fatalf("setup ids %d", len(ids))
	}
	before := packNames(t, repo)
	if len(before) == 0 {
		t.Fatal("repository has no packs")
	}

	wrong := executeRetentionNow(t, repo, "wrong-password", 1, 0, 0, 0)
	if wrong.result.status != StatusFailed || wrong.result.diagnostic != DiagAuthFailed {
		t.Fatalf("wrong key: %+v", wrong)
	}
	if !sameIDSet(snapshotIDSet(t, repo, password), ids) || !sameNameSet(packNames(t, repo), before) {
		t.Fatal("wrong key changed snapshots or packs")
	}

	control := filepath.Join(t.TempDir(), "control")
	hourlyTwo := filepath.Join(t.TempDir(), "hourly-two")
	keepNone := filepath.Join(t.TempDir(), "keep-none")
	copyDir(t, repo, control)
	copyDir(t, repo, hourlyTwo)
	copyDir(t, repo, keepNone)
	deleteSnapshotOnly(t, control, password, made["plain-old"].id)
	if !sameNameSet(packNames(t, control), before) {
		t.Fatal("removing a snapshot without prune removed a pack")
	}

	kept := executeRetentionNow(t, repo, password, 1, 0, 0, 0)
	if kept.err != nil || kept.result.status != StatusOk || kept.result.diagnostic != "" {
		t.Fatalf("retention: %+v %v", kept.result, kept.err)
	}
	got := snapshotIDSet(t, repo, password)
	for label, snap := range made {
		_, present := got[snap.id]
		if label == "plain-old" {
			if present {
				t.Fatal("hourly retention kept the older untagged snapshot")
			}
			continue
		}
		if !present {
			t.Fatalf("retention deleted %s", label)
		}
		restored := restoreSnapshotFiles(t, repo, source, password, snap.id)
		if !sameFiles(restored, snap.files) {
			t.Fatalf("restored bytes differ for %s", label)
		}
	}
	removed, err := executeRestore(context.Background(), &bridgeRequest{
		operation: OpRestore, password: []byte(password), repoPath: repo,
		sourcePath: source, restorePath: filepath.Join(t.TempDir(), "removed"), snapshotID: made["plain-old"].id,
	}, func() {})
	if removed.status == StatusOk {
		t.Fatalf("deleted snapshot restored: %v", err)
	}
	missingPack := false
	after := packNames(t, repo)
	for name := range before {
		if _, ok := after[name]; !ok {
			missingPack = true
		}
	}
	if !missingPack {
		t.Fatal("prune left every previous pack in place")
	}

	wider := executeRetentionNow(t, hourlyTwo, password, 2, 0, 0, 0)
	if wider.err != nil || wider.result.status != StatusOk {
		t.Fatalf("wider retention: %+v %v", wider.result, wider.err)
	}
	widerIDs := snapshotIDSet(t, hourlyTwo, password)
	if _, ok := widerIDs[made["plain-old"].id]; !ok || len(widerIDs) != len(made) {
		t.Fatal("hourly 2 did not keep the older untagged snapshot")
	}

	refused := executeRetentionNow(t, keepNone, password, 0, 0, 0, 0)
	if refused.result.status != StatusFailed || refused.result.diagnostic != DiagInvalidArgument {
		t.Fatalf("zero-keep policy: %+v %v", refused.result, refused.err)
	}
	if !sameIDSet(snapshotIDSet(t, keepNone, password), ids) {
		t.Fatal("refused policy deleted snapshots")
	}
}

func TestIntegrationRetentionFailureAfterMutation(t *testing.T) {
	repo, source, password, made := retentionRepository(t, false)
	oldID := made["plain-old"].id
	newID := made["plain-new"].id
	beforeIDs := snapshotIDSet(t, repo, password)

	testGate = func(stage string, _ context.Context) error {
		if stage == "retention-before-mutation" {
			return errors.New("stop before mutation")
		}
		return nil
	}
	t.Cleanup(func() { testGate = nil })
	early := executeRetentionNow(t, repo, password, 1, 0, 0, 0)
	testGate = nil
	if early.result.status != StatusCancelled || early.result.diagnostic != DiagCancelled {
		t.Fatalf("before mutation: %+v", early.result)
	}
	if !sameIDSet(snapshotIDSet(t, repo, password), beforeIDs) {
		t.Fatal("cancel before mutation deleted a snapshot")
	}

	testGate = func(stage string, _ context.Context) error {
		if stage == "retention-after-snapshot-delete" {
			return errors.New("stop after delete")
		}
		return nil
	}
	deleted := executeRetentionNow(t, repo, password, 1, 0, 0, 0)
	testGate = nil
	if deleted.result.status != StatusCancelledUncertain || deleted.result.diagnostic != DiagCancelledUncertain {
		t.Fatalf("after delete: %+v", deleted.result)
	}
	left := snapshotIDSet(t, repo, password)
	if _, ok := left[oldID]; ok {
		t.Fatal("deleted snapshot still listed")
	}
	if _, ok := left[newID]; !ok || !sameFiles(restoreSnapshotFiles(t, repo, source, password, newID), made["plain-new"].files) {
		t.Fatal("surviving snapshot did not restore")
	}

	repo, source, password, made = retentionRepository(t, true)
	oldID = made["plain-old"].id
	newID = made["plain-new"].id
	testGate = func(stage string, _ context.Context) error {
		if stage == "retention-during-prune" {
			return errors.New("stop during prune")
		}
		return nil
	}
	during := executeRetentionNow(t, repo, password, 1, 0, 0, 0)
	testGate = nil
	if during.result.status != StatusCancelledUncertain || during.result.diagnostic != DiagCancelledUncertain {
		t.Fatalf("during prune: %+v %v", during.result, during.err)
	}
	left = snapshotIDSet(t, repo, password)
	if _, ok := left[oldID]; ok {
		t.Fatal("prune interruption restored the deleted snapshot")
	}
	if _, ok := left[newID]; !ok || !sameFiles(restoreSnapshotFiles(t, repo, source, password, newID), made["plain-new"].files) {
		t.Fatal("snapshot kept across prune interruption did not restore")
	}
}

type retentionCall struct {
	result bridgeResult
	err    error
}

func executeRetentionNow(t *testing.T, repo, password string, hourly, daily, weekly, monthly int) retentionCall {
	t.Helper()
	result, err := executeRetention(context.Background(), bridgeRequest{
		operation: OpRetention,
		password:  []byte(password),
		repoPath:  repo,
		retentionPolicy: &retentionPolicy{
			hourly: hourly, daily: daily, weekly: weekly, monthly: monthly, tags: bothArchiveTags(),
		},
	}, func() {})
	return retentionCall{result: result, err: err}
}

func retentionRepository(t *testing.T, largeUnique bool) (repo, source, password string, made map[string]retainedSnapshot) {
	t.Helper()
	root := t.TempDir()
	repo = filepath.Join(root, "repo")
	source = filepath.Join(root, "source")
	password = "retention-daily-secret"
	if err := os.Mkdir(source, 0700); err != nil {
		t.Fatal(err)
	}
	initRes, err := executeInit(context.Background(), bridgeRequest{operation: OpInit, password: []byte(password), repoPath: repo}, func() {})
	if err != nil || initRes.status != StatusOk {
		t.Fatalf("init: %+v %v", initRes, err)
	}
	shared := []byte("shared-bytes")
	unique := []byte("unique-old")
	if largeUnique {
		unique = bytes.Repeat([]byte{0xab}, 1<<20)
	}
	steps := []struct {
		label string
		files map[string][]byte
		when  time.Time
		tags  []string
	}{
		{"plain-old", map[string][]byte{"shared.txt": shared, "unique-old.bin": unique}, at(2020, time.January, 1, 1, 0), nil},
		{"plain-new", map[string][]byte{"shared.txt": shared, "marker-new.txt": []byte("new")}, at(2020, time.January, 1, 5, 0), nil},
		{"archive-old", map[string][]byte{"shared.txt": shared, "marker-archive-old.txt": []byte("archive-old")}, at(2020, time.January, 2, 1, 0), []string{archiveKeepTag}},
		{"archive-new", map[string][]byte{"shared.txt": shared, "marker-archive-new.txt": []byte("archive-new")}, at(2020, time.January, 2, 5, 0), []string{archiveKeepTag}},
		{"offload-old", map[string][]byte{"shared.txt": shared, "marker-offload-old.txt": []byte("offload-old")}, at(2020, time.January, 3, 1, 0), []string{offloadKeepTag}},
		{"offload-new", map[string][]byte{"shared.txt": shared, "marker-offload-new.txt": []byte("offload-new")}, at(2020, time.January, 3, 5, 0), []string{offloadKeepTag}},
	}
	made = map[string]retainedSnapshot{}
	for _, step := range steps {
		replaceSource(t, source, step.files)
		snap, err := executeSnapshot(context.Background(), bridgeRequest{
			operation: OpSnapshot, password: []byte(password), repoPath: repo, sourcePath: source,
		}, func() {})
		if err != nil || snap.status != StatusOk || snap.snapshotID == "" {
			t.Fatalf("snapshot %s: %+v %v", step.label, snap, err)
		}
		id := rewriteSnapshot(t, repo, password, snap.snapshotID, step.when, step.tags)
		made[step.label] = retainedSnapshot{id: id, files: step.files}
	}
	return repo, source, password, made
}

func replaceSource(t *testing.T, source string, files map[string][]byte) {
	t.Helper()
	entries, err := os.ReadDir(source)
	if err != nil {
		t.Fatal(err)
	}
	for _, entry := range entries {
		if err := os.RemoveAll(filepath.Join(source, entry.Name())); err != nil {
			t.Fatal(err)
		}
	}
	for name, body := range files {
		if err := os.WriteFile(filepath.Join(source, name), body, 0600); err != nil {
			t.Fatal(err)
		}
	}
}

func rewriteSnapshot(t *testing.T, repoPath, password, id string, when time.Time, tags []string) string {
	t.Helper()
	ctx := context.Background()
	root, err := os.OpenRoot(repoPath)
	if err != nil {
		t.Fatal(err)
	}
	defer root.Close()
	be, err := newRootBackend(root)
	if err != nil {
		t.Fatal(err)
	}
	defer be.Close()
	repo, err := repository.New(guardedBackend{be}, repository.Options{})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := safeScanKey(ctx, repo, password); err != nil {
		t.Fatal(err)
	}
	unlock, lockCtx, err := lockRepo(ctx, repo, true)
	if err != nil {
		t.Fatal(err)
	}
	defer unlock()
	parsed, err := restic.ParseID(id)
	if err != nil {
		t.Fatal(err)
	}
	sn, err := data.LoadSnapshot(lockCtx, repo, parsed)
	if err != nil {
		t.Fatal(err)
	}
	sn.Time = when
	if len(tags) == 0 {
		sn.Tags = nil
	} else {
		sn.Tags = append([]string(nil), tags...)
	}
	newID, err := data.SaveSnapshot(lockCtx, repo, sn)
	if err != nil {
		t.Fatal(err)
	}
	if err := repo.RemoveUnpacked(lockCtx, restic.WriteableSnapshotFile, parsed); err != nil {
		t.Fatal(err)
	}
	return newID.String()
}

func deleteSnapshotOnly(t *testing.T, repoPath, password, id string) {
	t.Helper()
	ctx := context.Background()
	root, err := os.OpenRoot(repoPath)
	if err != nil {
		t.Fatal(err)
	}
	defer root.Close()
	be, err := newRootBackend(root)
	if err != nil {
		t.Fatal(err)
	}
	defer be.Close()
	repo, err := repository.New(guardedBackend{be}, repository.Options{})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := safeScanKey(ctx, repo, password); err != nil {
		t.Fatal(err)
	}
	unlock, lockCtx, err := lockRepo(ctx, repo, true)
	if err != nil {
		t.Fatal(err)
	}
	defer unlock()
	parsed, err := restic.ParseID(id)
	if err != nil {
		t.Fatal(err)
	}
	if err := repo.RemoveUnpacked(lockCtx, restic.WriteableSnapshotFile, parsed); err != nil {
		t.Fatal(err)
	}
}

func snapshotIDSet(t *testing.T, repo, password string) map[string]struct{} {
	t.Helper()
	res, err := executeListSnapshots(context.Background(), bridgeRequest{operation: OpListSnapshots, password: []byte(password), repoPath: repo})
	if err != nil || res.status != StatusOk {
		t.Fatalf("list: %+v %v", res, err)
	}
	out := map[string]struct{}{}
	for _, id := range res.snapshotIDs {
		out[id] = struct{}{}
	}
	return out
}

func sameIDSet(got, want map[string]struct{}) bool {
	if len(got) != len(want) {
		return false
	}
	for id := range want {
		if _, ok := got[id]; !ok {
			return false
		}
	}
	return true
}

func packNames(t *testing.T, repo string) map[string]struct{} {
	t.Helper()
	out := map[string]struct{}{}
	root := filepath.Join(repo, "data")
	err := filepath.WalkDir(root, func(path string, entry os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if entry.IsDir() {
			return nil
		}
		rel, err := filepath.Rel(root, path)
		if err != nil {
			return err
		}
		out[rel] = struct{}{}
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	return out
}

func sameNameSet(got, want map[string]struct{}) bool {
	return sameIDSet(got, want)
}

func copyDir(t *testing.T, src, dst string) {
	t.Helper()
	err := filepath.WalkDir(src, func(path string, entry os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		rel, err := filepath.Rel(src, path)
		if err != nil {
			return err
		}
		target := filepath.Join(dst, rel)
		if entry.IsDir() {
			return os.MkdirAll(target, 0700)
		}
		in, err := os.Open(path)
		if err != nil {
			return err
		}
		defer in.Close()
		if err := os.MkdirAll(filepath.Dir(target), 0700); err != nil {
			return err
		}
		out, err := os.OpenFile(target, os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0600)
		if err != nil {
			return err
		}
		defer out.Close()
		_, err = io.Copy(out, in)
		return err
	})
	if err != nil {
		t.Fatal(err)
	}
}

func restoreSnapshotDir(t *testing.T, repo, source, password, id string) string {
	t.Helper()
	dest := filepath.Join(t.TempDir(), "restore")
	req := bridgeRequest{
		operation: OpRestore, password: []byte(password), repoPath: repo,
		sourcePath: source, restorePath: dest, snapshotID: id,
	}
	res, err := executeRestore(context.Background(), &req, func() {})
	if err != nil || res.status != StatusOk {
		t.Fatalf("restore %s: %+v %v", id, res, err)
	}
	return filepath.Join(dest, filepath.Base(source))
}

func restoreSnapshotFiles(t *testing.T, repo, source, password, id string) map[string][]byte {
	t.Helper()
	dir := restoreSnapshotDir(t, repo, source, password, id)
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatal(err)
	}
	out := map[string][]byte{}
	for _, entry := range entries {
		if entry.IsDir() {
			t.Fatalf("unexpected directory %s", entry.Name())
		}
		body, err := os.ReadFile(filepath.Join(dir, entry.Name()))
		if err != nil {
			t.Fatal(err)
		}
		out[entry.Name()] = body
	}
	return out
}

func sameFiles(got, want map[string][]byte) bool {
	if len(got) != len(want) {
		return false
	}
	for name, body := range want {
		if !bytes.Equal(got[name], body) {
			return false
		}
	}
	return true
}
