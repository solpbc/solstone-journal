// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

import (
	"bytes"
	"context"
	"encoding/binary"
	"errors"
	"net/http"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/restic/restic/internal/backend"
	"github.com/restic/restic/internal/data"
	"github.com/restic/restic/internal/ui"
)

func at(year int, month time.Month, day, hour, minute int) time.Time {
	return time.Date(year, month, day, hour, minute, 0, 0, time.UTC)
}

func labeled(label, host, path string, when time.Time, tags ...string) *data.Snapshot {
	var copied []string
	if len(tags) > 0 {
		copied = append([]string(nil), tags...)
	}
	return &data.Snapshot{Time: when, Hostname: host, Paths: []string{path}, Tags: copied, Username: label}
}

func sameLabels(got map[string]struct{}, want ...string) bool {
	if len(got) != len(want) {
		return false
	}
	for _, label := range want {
		if _, ok := got[label]; !ok {
			return false
		}
	}
	return true
}

func splitLabels(t *testing.T, list data.Snapshots, policy data.ExpirePolicy) (keep, drop map[string]struct{}) {
	t.Helper()
	kept, removed, _ := data.ApplyPolicy(append(data.Snapshots(nil), list...), policy)
	keep, drop = map[string]struct{}{}, map[string]struct{}{}
	for _, sn := range kept {
		keep[sn.Username] = struct{}{}
	}
	for _, sn := range removed {
		drop[sn.Username] = struct{}{}
	}
	if len(keep) == 0 || len(drop) == 0 {
		t.Fatalf("selector kept all or none: keep %v drop %v", keep, drop)
	}
	return keep, drop
}

func groupedLabels(t *testing.T, snaps []*data.Snapshot, by data.SnapshotGroupByOptions, policy data.ExpirePolicy) (keep, drop map[string]struct{}) {
	t.Helper()
	groups, _, err := data.GroupSnapshots(append(data.Snapshots(nil), snaps...), by)
	if err != nil {
		t.Fatal(err)
	}
	keep, drop = map[string]struct{}{}, map[string]struct{}{}
	for _, group := range groups {
		kept, removed, _ := data.ApplyPolicy(group, policy)
		for _, sn := range kept {
			keep[sn.Username] = struct{}{}
		}
		for _, sn := range removed {
			drop[sn.Username] = struct{}{}
		}
	}
	return keep, drop
}

func TestRetentionBucketsKeepAProperSubset(t *testing.T) {
	hourly := data.Snapshots{
		labeled("hour-1", "h", "/a", at(2020, time.April, 1, 1, 0)),
		labeled("hour-1-later", "h", "/a", at(2020, time.April, 1, 1, 30)),
		labeled("hour-2", "h", "/a", at(2020, time.April, 1, 2, 0)),
	}
	keep, drop := splitLabels(t, hourly, data.ExpirePolicy{Hourly: 2})
	if !sameLabels(keep, "hour-2", "hour-1-later") || !sameLabels(drop, "hour-1") {
		t.Fatalf("hourly buckets: keep %v drop %v", keep, drop)
	}

	daily := data.Snapshots{
		labeled("day-old", "h", "/a", at(2020, time.April, 1, 1, 0)),
		labeled("day-new", "h", "/a", at(2020, time.April, 2, 1, 0)),
	}
	keep, drop = splitLabels(t, daily, data.ExpirePolicy{Daily: 1})
	if !sameLabels(keep, "day-new") || !sameLabels(drop, "day-old") {
		t.Fatalf("daily buckets: keep %v drop %v", keep, drop)
	}

	weekly := data.Snapshots{
		labeled("week-1", "h", "/a", at(2020, time.January, 1, 1, 0)),
		labeled("week-2", "h", "/a", at(2020, time.January, 8, 1, 0)),
	}
	keep, drop = splitLabels(t, weekly, data.ExpirePolicy{Weekly: 1})
	if !sameLabels(keep, "week-2") || !sameLabels(drop, "week-1") {
		t.Fatalf("weekly buckets: keep %v drop %v", keep, drop)
	}

	monthly := data.Snapshots{
		labeled("january", "h", "/a", at(2020, time.January, 15, 1, 0)),
		labeled("february", "h", "/a", at(2020, time.February, 15, 1, 0)),
	}
	keep, drop = splitLabels(t, monthly, data.ExpirePolicy{Monthly: 1})
	if !sameLabels(keep, "february") || !sameLabels(drop, "january") {
		t.Fatalf("monthly buckets: keep %v drop %v", keep, drop)
	}

	// One newest snapshot consumes every positive count at once.
	months := data.Snapshots{
		labeled("m1", "h", "/a", at(2020, time.January, 15, 1, 0)),
		labeled("m2", "h", "/a", at(2020, time.February, 15, 1, 0)),
		labeled("m3", "h", "/a", at(2020, time.March, 15, 1, 0)),
		labeled("m4", "h", "/a", at(2020, time.April, 15, 1, 0)),
	}
	keep, drop = splitLabels(t, months, data.ExpirePolicy{Hourly: 1, Daily: 1, Weekly: 1, Monthly: 1})
	if !sameLabels(keep, "m4") || !sameLabels(drop, "m1", "m2", "m3") {
		t.Fatalf("combined counts: keep %v drop %v", keep, drop)
	}

	// A large hourly count still keeps one snapshot per hour, plus the oldest while a count remains.
	sameHour := data.Snapshots{
		labeled("oldest", "h", "/a", at(2020, time.April, 1, 1, 0)),
		labeled("middle", "h", "/a", at(2020, time.April, 1, 1, 20)),
		labeled("newest", "h", "/a", at(2020, time.April, 1, 1, 40)),
	}
	keep, drop = splitLabels(t, sameHour, data.ExpirePolicy{Hourly: 10})
	if !sameLabels(keep, "newest", "oldest") || !sameLabels(drop, "middle") {
		t.Fatalf("same-hour count: keep %v drop %v", keep, drop)
	}
}

func bothArchiveTags() []data.TagList {
	return []data.TagList{{archiveKeepTag}, {offloadKeepTag}}
}

func variedHostSnapshots() []*data.Snapshot {
	return []*data.Snapshot{
		labeled("plain-old", "host-a", "/journal/a", at(2020, time.January, 1, 1, 0)),
		labeled("plain-new", "host-b", "/journal/b", at(2020, time.January, 1, 5, 0)),
		labeled("archive-old", "host-c", "/journal/c", at(2020, time.January, 2, 1, 0), archiveKeepTag),
		labeled("archive-new", "host-d", "/journal/d", at(2020, time.January, 2, 5, 0), archiveKeepTag),
		labeled("offload-old", "host-e", "/journal/e", at(2020, time.January, 3, 1, 0), offloadKeepTag),
		labeled("offload-new", "host-f", "/journal/f", at(2020, time.January, 3, 5, 0), offloadKeepTag),
	}
}

func TestRetentionTagGroupingKeepsEitherArchiveTag(t *testing.T) {
	snaps := variedHostSnapshots()
	policy := data.ExpirePolicy{Hourly: 1, Tags: bothArchiveTags()}
	keep, drop := groupedLabels(t, snaps, data.SnapshotGroupByOptions{Tag: true}, policy)
	if len(keep) == 0 || len(drop) == 0 || !sameLabels(keep, "plain-new", "archive-old", "archive-new", "offload-old", "offload-new") || !sameLabels(drop, "plain-old") {
		t.Fatalf("tagged grouping: keep %v drop %v", keep, drop)
	}

	withoutArchive := data.ExpirePolicy{Hourly: 1, Tags: []data.TagList{{offloadKeepTag}}}
	keepArchive, _ := groupedLabels(t, snaps, data.SnapshotGroupByOptions{Tag: true}, withoutArchive)
	if sameLabels(keepArchive, "plain-new", "archive-old", "archive-new", "offload-old", "offload-new") || !sameLabels(keepArchive, "plain-new", "archive-new", "offload-old", "offload-new") {
		t.Fatalf("omitting the archive tag kept the same set: %v", keepArchive)
	}
	withoutOffload := data.ExpirePolicy{Hourly: 1, Tags: []data.TagList{{archiveKeepTag}}}
	keepOffload, _ := groupedLabels(t, snaps, data.SnapshotGroupByOptions{Tag: true}, withoutOffload)
	if sameLabels(keepOffload, "plain-new", "archive-old", "archive-new", "offload-old", "offload-new") || !sameLabels(keepOffload, "plain-new", "archive-old", "archive-new", "offload-new") {
		t.Fatalf("omitting the offload tag kept the same set: %v", keepOffload)
	}

	byHost := groupedLabelsKeep(t, snaps, data.SnapshotGroupByOptions{Host: true, Path: true}, policy)
	if sameLabels(byHost, "plain-new", "archive-old", "archive-new", "offload-old", "offload-new") || !sameLabels(byHost, "plain-old", "plain-new", "archive-old", "archive-new", "offload-old", "offload-new") {
		t.Fatalf("host and path grouping did not keep each newest snapshot: %v", byHost)
	}

	untagged := []*data.Snapshot{
		labeled("u1", "alpha", "/one", at(2020, time.May, 1, 1, 0)),
		labeled("u2", "beta", "/two", at(2020, time.May, 1, 2, 0)),
		labeled("u3", "gamma", "/three", at(2020, time.May, 1, 3, 0)),
	}
	tagKeep, tagDrop := groupedLabels(t, untagged, data.SnapshotGroupByOptions{Tag: true}, data.ExpirePolicy{Hourly: 1})
	hostKeep := groupedLabelsKeep(t, untagged, data.SnapshotGroupByOptions{Host: true, Path: true}, data.ExpirePolicy{Hourly: 1})
	if !sameLabels(tagKeep, "u3") || !sameLabels(tagDrop, "u1", "u2") || !sameLabels(hostKeep, "u1", "u2", "u3") {
		t.Fatalf("ungrouped hosts: tag %v %v host %v", tagKeep, tagDrop, hostKeep)
	}
}

func groupedLabelsKeep(t *testing.T, snaps []*data.Snapshot, by data.SnapshotGroupByOptions, policy data.ExpirePolicy) map[string]struct{} {
	t.Helper()
	keep, _ := groupedLabels(t, snaps, by, policy)
	return keep
}

func TestRetentionTagsMatchAcrossListsAndInsideOneList(t *testing.T) {
	list := data.Snapshots{
		labeled("archive", "h", "/a", at(2020, time.June, 1, 1, 0), archiveKeepTag),
		labeled("offload", "h", "/a", at(2020, time.June, 1, 2, 0), offloadKeepTag),
		labeled("both", "h", "/a", at(2020, time.June, 1, 3, 0), archiveKeepTag, "extra"),
		labeled("other", "h", "/a", at(2020, time.June, 1, 4, 0), "other"),
	}
	keep, drop := splitLabels(t, list, data.ExpirePolicy{Tags: bothArchiveTags()})
	if !sameLabels(keep, "archive", "offload", "both") || !sameLabels(drop, "other") {
		t.Fatalf("tag or: keep %v drop %v", keep, drop)
	}
	andList := data.Snapshots{
		labeled("one", "h", "/a", at(2020, time.June, 2, 1, 0), archiveKeepTag),
		labeled("pair", "h", "/a", at(2020, time.June, 2, 2, 0), archiveKeepTag, "must-both"),
	}
	keep, drop = splitLabels(t, andList, data.ExpirePolicy{Tags: []data.TagList{{archiveKeepTag, "must-both"}}})
	if !sameLabels(keep, "pair") || !sameLabels(drop, "one") {
		t.Fatalf("tag and: keep %v drop %v", keep, drop)
	}
}

func retentionBlob(hourly, daily, weekly, monthly uint64, tags []string) []byte {
	var buf bytes.Buffer
	buf.WriteString(retentionMagic)
	for _, count := range []uint64{hourly, daily, weekly, monthly} {
		_ = binary.Write(&buf, binary.LittleEndian, count)
	}
	_ = binary.Write(&buf, binary.LittleEndian, uint32(len(tags)))
	for _, tag := range tags {
		_ = binary.Write(&buf, binary.LittleEndian, uint16(len(tag)))
		buf.WriteString(tag)
	}
	return buf.Bytes()
}

func TestRetentionPolicyBlobPreservesCountsAndTags(t *testing.T) {
	policy, status := parseRetentionPolicy(retentionBlob(0, 7, 4, 12, []string{archiveKeepTag, offloadKeepTag}))
	if status != StatusOk || policy.hourly != 0 || policy.daily != 7 || policy.weekly != 4 || policy.monthly != 12 || !exactArchiveTags(policy.tags) {
		t.Fatalf("blob decode: %+v %d", policy, status)
	}
	for _, buf := range [][]byte{
		retentionBlob(1, 1, 1, 1, []string{archiveKeepTag}),
		retentionBlob(1, 1, 1, 1, []string{offloadKeepTag, archiveKeepTag}),
		retentionBlob(1, 1, 1, 1, []string{archiveKeepTag, archiveKeepTag}),
		append(retentionBlob(1, 1, 1, 1, []string{archiveKeepTag, offloadKeepTag}), 0),
		{0, 0, 0, 0},
	} {
		if _, status := parseRetentionPolicy(buf); status != StatusInvalidArgument {
			t.Fatal("malformed retention blob accepted")
		}
	}
	huge := retentionBlob(^uint64(0), 0, 0, 0, []string{archiveKeepTag, offloadKeepTag})
	if _, status := parseRetentionPolicy(huge); status != StatusInvalidArgument {
		t.Fatal("overflowing count accepted")
	}
	if _, ok := acceptRetentionPolicy(nil); ok {
		t.Fatal("nil policy accepted")
	}
	if _, ok := acceptRetentionPolicy(&retentionPolicy{}); ok {
		t.Fatal("empty policy accepted")
	}
	if _, ok := acceptRetentionPolicy(&retentionPolicy{hourly: 1, tags: []data.TagList{{archiveKeepTag}}}); ok {
		t.Fatal("one-tag policy accepted")
	}
	if _, ok := acceptRetentionPolicy(&retentionPolicy{hourly: -1, tags: bothArchiveTags()}); ok {
		t.Fatal("keep-all count accepted")
	}
}

func TestRetentionPruneUsesOneGigabyteAndDefaultUnusedLimit(t *testing.T) {
	size, err := ui.ParseBytes("1G")
	if err != nil || size != 1073741824 {
		t.Fatalf("1G parsed as %d (%v)", size, err)
	}
	opts, err := retentionPruneOptions()
	if err != nil || opts.MaxRepackBytes != uint64(size) || opts.MaxRepackBytes == ^uint64(0) || opts.MaxUnusedBytes == nil {
		t.Fatal("prune options drifted", err)
	}
	if opts.MaxUnusedBytes(95) != 5 || opts.DryRun || opts.UnsafeRecovery {
		t.Fatal("unused limit or extra prune flags drifted")
	}
}

func TestRetentionRefusesBeforeRepositoryMutation(t *testing.T) {
	dir := t.TempDir()
	wrote := false
	mark := func() { wrote = true }
	cases := []bridgeRequest{
		{operation: OpRetention, password: []byte("secret"), repoPath: dir},
		{operation: OpRetention, password: []byte("secret"), repoPath: dir, retentionPolicy: &retentionPolicy{}},
		{operation: OpRetention, password: []byte("secret"), repoPath: dir, retentionPolicy: &retentionPolicy{hourly: 1, tags: bothArchiveTags()}, executionOptions: &operationOptions{appendOnly: true}},
	}
	for _, req := range cases {
		wrote = false
		result, err := executeRetention(context.Background(), req, mark)
		if wrote || err == nil || result.status != StatusInvalidArgument || result.diagnostic != DiagInvalidArgument {
			t.Fatalf("policy reached storage: %+v %v", result, err)
		}
	}
	if _, err := os.Stat(filepath.Join(dir, "config")); !os.IsNotExist(err) {
		t.Fatal("refused retention created a repository")
	}
}

func TestRetentionShapeRejectsExtraFieldsAndUnknownOpcodes(t *testing.T) {
	valid := requestShape{operation: OpRetention, fields: [6]requestField{{4, true}, {0, false}, {8, true}}}
	if validateShape(valid) != StatusOk {
		t.Fatal("retention shape refused")
	}
	for _, index := range []int{1, 3, 4, 5} {
		bad := valid
		bad.fields[index] = requestField{1, true}
		if validateShape(bad) != StatusInvalidArgument {
			t.Fatalf("retention field %d accepted", index)
		}
	}
	for _, op := range []int32{8, 11, 13, 20} {
		bad := valid
		bad.operation = op
		if validateShape(bad) != StatusInvalidArgument {
			t.Fatalf("opcode %d accepted as a local operation", op)
		}
	}
	if adoptsUncertainty(OpRetention) {
		t.Fatal("retention adopted content-addressed uncertainty")
	}
}

func TestS3BackendRetentionScopeRemovals(t *testing.T) {
	if isRetentionScope(withTeardownScope(context.Background())) || isTeardownScope(withRetentionScope(context.Background())) {
		t.Fatal("retention and teardown scopes are not distinct")
	}
	transport := s3TestTransport(func(req *http.Request) (*http.Response, error) {
		if req.Method == http.MethodDelete {
			return s3TestResponse(http.StatusNoContent, "", nil), nil
		}
		return s3TestResponse(http.StatusOK, "", nil), nil
	})
	be := s3TestBackend(t, s3TestConfig(), transport)
	ctx := context.Background()
	retentionCtx := withRetentionScope(ctx)
	if err := be.Remove(ctx, backend.Handle{Type: backend.PackFile, Name: s3TestID(1)}); !errors.Is(err, errS3Denied) {
		t.Fatal("unscoped removal was allowed", err)
	}
	for _, handleType := range []backend.FileType{backend.SnapshotFile, backend.PackFile, backend.IndexFile} {
		if err := be.Remove(retentionCtx, backend.Handle{Type: handleType, Name: s3TestID(2)}); err != nil {
			t.Fatalf("retention removal of %v refused: %v", handleType, err)
		}
	}
	for _, handleType := range []backend.FileType{backend.ConfigFile, backend.KeyFile} {
		handle := backend.Handle{Type: handleType, Name: s3TestID(3)}
		if handleType == backend.ConfigFile {
			handle.Name = ""
		}
		if err := be.Remove(retentionCtx, handle); !errors.Is(err, errS3Denied) {
			t.Fatalf("retention removal of %v was allowed: %v", handleType, err)
		}
	}
	if err := be.Delete(retentionCtx); !errors.Is(err, errS3Denied) {
		t.Fatal("retention scope allowed backend delete", err)
	}
}

func TestS3OwnerRetentionAttemptStaysPending(t *testing.T) {
	cfg, root := s3TestConfig(), s3OwnerTestDirectory(t)
	owner := s3OwnerTestOpen(t, root, cfg)
	if owner.begin(OpTeardown, false, cfg, []byte("daily")) == nil {
		t.Fatal("teardown entered ordinary begin")
	}
	if owner.begin(OpRetention, false, cfg, []byte("daily")) != nil || owner.noteSave(s3TestID(2)) != nil {
		t.Fatal("retention intent failed")
	}
	owner.lockRemoved(s3TestID(2), nil)
	if owner.drainPrimary() != nil {
		t.Fatal("drain failed")
	}
	owner.noteAttemptFailed()
	if published, err := owner.complete(); published || !errors.Is(err, errS3OwnerPending) {
		t.Fatal("acked retention attempt completed", err)
	}
	s3OwnerTestClose(t, owner)
	reopened := s3OwnerTestOpen(t, root, cfg)
	if reopened.record.Active == nil || !reopened.record.Active.MayHaveWritten || reopened.record.Active.Operation != OpRetention {
		t.Fatal("pending retention attempt was dropped")
	}
	before, err := os.ReadFile(filepath.Join(root, s3OwnerStateName))
	if err != nil {
		t.Fatal(err)
	}
	if err := reopened.begin(OpRetention, false, cfg, []byte("daily")); !errors.Is(err, errS3OwnerPending) {
		t.Fatal("retry cleared a pending retention attempt", err)
	}
	after, err := os.ReadFile(filepath.Join(root, s3OwnerStateName))
	if err != nil || !bytes.Equal(before, after) {
		t.Fatal("pending refusal changed owner state")
	}
}
