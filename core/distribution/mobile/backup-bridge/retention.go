// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

import (
	"context"
	"encoding/binary"
	"errors"
	"math"

	"github.com/restic/restic/internal/backend"
	"github.com/restic/restic/internal/data"
	"github.com/restic/restic/internal/repository"
	"github.com/restic/restic/internal/restic"
	"github.com/restic/restic/internal/ui"
	"github.com/restic/restic/internal/ui/progress"
)

const (
	retentionMagic = "RTN1"
	archiveKeepTag = "solstone-archive"
	offloadKeepTag = "spb-offload-archive"
)

// Closed counts plus the two archive tags. An empty or other shape never reaches storage.
type retentionPolicy struct {
	hourly  int
	daily   int
	weekly  int
	monthly int
	tags    []data.TagList
}

func parseRetentionPolicy(buf []byte) (*retentionPolicy, int32) {
	want := []string{archiveKeepTag, offloadKeepTag}
	size := 4 + 8*4 + 4
	for _, tag := range want {
		size += 2 + len(tag)
	}
	if len(buf) != size || string(buf[:4]) != retentionMagic {
		return nil, StatusInvalidArgument
	}
	off := 4
	counts := [4]int{}
	for i := 0; i < 4; i++ {
		value := binary.LittleEndian.Uint64(buf[off : off+8])
		off += 8
		if value > uint64(math.MaxInt) {
			return nil, StatusInvalidArgument
		}
		counts[i] = int(value)
	}
	if binary.LittleEndian.Uint32(buf[off:off+4]) != 2 {
		return nil, StatusInvalidArgument
	}
	off += 4
	tags := make([]data.TagList, 0, 2)
	for _, tag := range want {
		if off+2 > len(buf) {
			return nil, StatusInvalidArgument
		}
		n := int(binary.LittleEndian.Uint16(buf[off : off+2]))
		off += 2
		if n != len(tag) || off+n > len(buf) || string(buf[off:off+n]) != tag {
			return nil, StatusInvalidArgument
		}
		off += n
		tags = append(tags, data.TagList{tag})
	}
	if off != len(buf) {
		return nil, StatusInvalidArgument
	}
	return &retentionPolicy{
		hourly: counts[0], daily: counts[1], weekly: counts[2], monthly: counts[3], tags: tags,
	}, StatusOk
}

func (p *retentionPolicy) expire() data.ExpirePolicy {
	if p == nil {
		return data.ExpirePolicy{}
	}
	return data.ExpirePolicy{
		Hourly:  p.hourly,
		Daily:   p.daily,
		Weekly:  p.weekly,
		Monthly: p.monthly,
		Tags:    p.tags,
	}
}

func exactArchiveTags(tags []data.TagList) bool {
	if len(tags) != 2 || len(tags[0]) != 1 || len(tags[1]) != 1 {
		return false
	}
	return tags[0][0] == archiveKeepTag && tags[1][0] == offloadKeepTag
}

func acceptRetentionPolicy(p *retentionPolicy) (data.ExpirePolicy, bool) {
	if p == nil || p.hourly < 0 || p.daily < 0 || p.weekly < 0 || p.monthly < 0 {
		return data.ExpirePolicy{}, false
	}
	policy := p.expire()
	if policy.Empty() || !exactArchiveTags(policy.Tags) {
		return data.ExpirePolicy{}, false
	}
	return policy, true
}

// retentionConnections reports the two connections PlanPrune requires.
// The local root backend reports one; this wrapper is used only for retention.
type retentionConnections struct{ backend.Backend }

func (b retentionConnections) Properties() backend.Properties {
	props := b.Backend.Properties()
	if props.Connections < 2 {
		props.Connections = 2
	}
	return props
}

// Desktop prune passes max-repack-size 1G and leaves restic's five-percent unused limit.
func retentionPruneOptions() (repository.PruneOptions, error) {
	size, err := ui.ParseBytes("1G")
	if err != nil || size <= 0 {
		return repository.PruneOptions{}, errors.New("repack bound refused")
	}
	return repository.PruneOptions{
		MaxUnusedBytes: func(used uint64) uint64 {
			return uint64(5.0 / 95.0 * float64(used))
		},
		MaxRepackBytes: uint64(size),
	}, nil
}

var errRetentionKeepEmpty = errors.New("retention would remove every snapshot in a group")

func loadRetentionSnapshots(ctx context.Context, repo *repository.Repository) (data.Snapshots, error) {
	var snapshots data.Snapshots
	err := data.ForAllSnapshots(ctx, repo, repo, nil, func(_ restic.ID, sn *data.Snapshot, err error) error {
		if err != nil {
			return err
		}
		if sn == nil || sn.ID() == nil {
			return errors.New("snapshot id missing")
		}
		snapshots = append(snapshots, sn)
		return nil
	})
	return snapshots, err
}

func selectRetentionRemovals(snapshots data.Snapshots, policy data.ExpirePolicy) ([]restic.ID, error) {
	groups, _, err := data.GroupSnapshots(snapshots, data.SnapshotGroupByOptions{Tag: true})
	if err != nil {
		return nil, err
	}
	var remove []restic.ID
	for _, group := range groups {
		keep, drop, _ := data.ApplyPolicy(group, policy)
		if len(keep) == 0 {
			return nil, errRetentionKeepEmpty
		}
		for _, sn := range drop {
			id := sn.ID()
			if id == nil {
				return nil, errors.New("snapshot id missing")
			}
			remove = append(remove, *id)
		}
	}
	return remove, nil
}

func retentionUsedBlobs(ignore restic.IDSet) func(context.Context, restic.Repository, restic.FindBlobSet) error {
	return func(ctx context.Context, r restic.Repository, usedBlobs restic.FindBlobSet) error {
		var snapshotTrees restic.IDs
		err := data.ForAllSnapshots(ctx, r, r, ignore, func(_ restic.ID, sn *data.Snapshot, err error) error {
			if err != nil {
				return err
			}
			if sn.Tree == nil {
				return errors.New("missing snapshot tree")
			}
			snapshotTrees = append(snapshotTrees, *sn.Tree)
			return nil
		})
		if err != nil {
			return err
		}
		counter := (&progress.NoopPrinter{}).NewCounter("snapshots")
		return data.FindUsedBlobs(ctx, r, snapshotTrees, usedBlobs, counter)
	}
}

func retentionAbort(wrote bool, err error) (bridgeResult, error) {
	cancelled := errors.Is(err, context.Canceled) || errors.Is(err, context.DeadlineExceeded)
	if wrote && cancelled {
		return bridgeResult{status: StatusCancelledUncertain, diagnostic: DiagCancelledUncertain}, err
	}
	if !wrote && cancelled {
		return bridgeResult{status: StatusCancelled, diagnostic: DiagCancelled}, err
	}
	return bridgeResult{status: StatusFailed, diagnostic: DiagIoFailed}, err
}

func retentionGate(stage string, ctx context.Context, wrote bool) (bridgeResult, error, bool) {
	var err error
	if testGate != nil {
		err = testGate(stage, ctx)
	}
	if err == nil {
		err = ctx.Err()
	}
	if err == nil {
		return bridgeResult{}, nil, false
	}
	if wrote {
		return bridgeResult{status: StatusCancelledUncertain, diagnostic: DiagCancelledUncertain}, err, true
	}
	return bridgeResult{status: StatusCancelled, diagnostic: DiagCancelled}, err, true
}

func executeRetention(ctx context.Context, req bridgeRequest, markWrite func()) (bridgeResult, error) {
	policy, ok := acceptRetentionPolicy(req.retentionPolicy)
	if !ok {
		return bridgeResult{status: StatusInvalidArgument, diagnostic: DiagInvalidArgument}, errors.New("retention policy refused")
	}
	if req.executionOptions != nil && req.executionOptions.appendOnly {
		return bridgeResult{status: StatusInvalidArgument, diagnostic: DiagInvalidArgument}, errors.New("append-only retention refused")
	}
	pruneOptions, err := retentionPruneOptions()
	if err != nil {
		return bridgeResult{status: StatusFailed, diagnostic: DiagInternal}, err
	}
	if !req.pathsPinned {
		if err := pinRequestPaths(&req); err != nil {
			return mapPinError(req.operation, err), err
		}
		defer closeRequestPaths(&req)
	}

	pw, clearPw := passwordString(req.password)
	defer clearPw()

	be, err := operationBackend(ctx, req, false)
	if err != nil {
		return bridgeResult{status: StatusFailed, diagnostic: DiagRepoMissing}, err
	}
	defer be.Close()

	repo, err := repository.New(guardedBackend{retentionConnections{be}}, repository.Options{})
	if err != nil {
		return bridgeResult{status: StatusFailed, diagnostic: DiagInternal}, err
	}
	if _, err = safeScanKey(ctx, repo, pw); err != nil {
		if ctx.Err() != nil {
			return bridgeResult{status: StatusCancelled, diagnostic: DiagCancelled}, ctx.Err()
		}
		return keyRefusal(ctx, be, err), err
	}
	unlock, lockCtx, err := lockRepoForRequest(ctx, req, repo, true)
	if err != nil {
		return bridgeResult{status: StatusFailed, diagnostic: DiagLocked}, err
	}
	defer unlock()

	snapshots, err := loadRetentionSnapshots(lockCtx, repo)
	if err != nil {
		return retentionAbort(false, err)
	}
	remove, err := selectRetentionRemovals(snapshots, policy)
	if errors.Is(err, errRetentionKeepEmpty) {
		return bridgeResult{status: StatusFailed, diagnostic: DiagInvalidArgument}, err
	}
	if err != nil {
		return retentionAbort(false, err)
	}
	if result, err, stop := retentionGate("retention-before-mutation", lockCtx, false); stop {
		return result, err
	}

	// Scope starts only after the policy, key, and lock have been accepted.
	lockCtx = withRetentionScope(lockCtx)
	wrote := false
	for i, id := range remove {
		if !wrote {
			markWrite()
			wrote = true
		}
		if err := repo.RemoveUnpacked(lockCtx, restic.WriteableSnapshotFile, id); err != nil {
			return retentionAbort(true, err)
		}
		if i == 0 {
			if result, err, stop := retentionGate("retention-after-snapshot-delete", lockCtx, true); stop {
				return result, err
			}
		}
	}

	if err := repo.LoadIndex(lockCtx, &progress.NoopPrinter{}); err != nil {
		return retentionAbort(wrote, err)
	}
	ignore := restic.NewIDSet()
	for _, id := range remove {
		ignore.Insert(id)
	}
	plan, err := repository.PlanPrune(lockCtx, pruneOptions, repo, retentionUsedBlobs(ignore), &progress.NoopPrinter{})
	if err != nil {
		return retentionAbort(wrote, err)
	}
	if result, err, stop := retentionGate("retention-during-prune", lockCtx, wrote); stop {
		return result, err
	}
	// A failure inside Execute must not report success, including when no snapshot was removed.
	markWrite()
	if err := plan.Execute(lockCtx, &progress.NoopPrinter{}); err != nil {
		return retentionAbort(true, err)
	}
	if err := lockCtx.Err(); err != nil {
		return bridgeResult{status: StatusCancelledUncertain, diagnostic: DiagCancelledUncertain}, err
	}
	return bridgeResult{status: StatusOk}, nil
}
