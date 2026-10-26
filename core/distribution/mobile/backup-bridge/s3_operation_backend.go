// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

import (
	"context"
	"errors"
	"io"
	"sync"

	"github.com/restic/restic/internal/backend"
)

// Every actual backend carries the specific request's owner, never ambient
// credential or invocation authority. Cleanup uses a distinct delete-only value.
type s3OperationBackend struct {
	*s3Backend
	owner     *s3Owner
	cleanup   bool
	mu        sync.Mutex
	sealed    bool
	active    sync.WaitGroup
	afterSave func()
}

var errS3InitExisting = errors.New("S3_INIT_EXISTING_OBJECTS")

// Recognize only the pinned Init key/snapshot callbacks before the private
// backend scrubs consumer errors. No arbitrary callback text crosses that seam.
func (be *s3OperationBackend) List(ctx context.Context, fileType backend.FileType, consume func(backend.FileInfo) error) error {
	done, err := be.enterTracked()
	if err != nil {
		return err
	}
	defer done()
	be.owner.mu.Lock()
	initializing := be.owner.record.Active != nil && be.owner.record.Active.Operation == OpInit && be.owner.liveID == be.owner.record.Active.ID
	be.owner.mu.Unlock()
	return be.s3Backend.List(ctx, fileType, func(info backend.FileInfo) error {
		err := consume(info)
		if initializing && err != nil && (fileType == backend.KeyFile && err.Error() == "repository already contains keys" || fileType == backend.SnapshotFile && err.Error() == "repository already contains snapshots") {
			return errS3InitExisting
		}
		return err
	})
}

func (owner *s3Owner) attachBackend(be *s3Backend) (*s3OperationBackend, error) {
	owner.mu.Lock()
	if owner.closed || owner.sealing || owner.record.Active == nil || owner.stateFailed || owner.liveID == "" || owner.liveID != owner.record.Active.ID {
		owner.mu.Unlock()
		_ = owner.owned.close(be, "s3-rejected-backend")
		return nil, errS3OwnerInvalid
	}
	wrapped := &s3OperationBackend{s3Backend: be, owner: owner}
	owner.backends = append(owner.backends, wrapped)
	owner.mu.Unlock()
	return wrapped, nil
}

func (be *s3OperationBackend) enterTracked() (func(), error) {
	be.mu.Lock()
	defer be.mu.Unlock()
	if be.sealed {
		return nil, errS3Closed
	}
	be.active.Add(1)
	return be.active.Done, nil
}

func (be *s3OperationBackend) Close() error {
	if be.owner.owned != nil {
		return be.owner.owned.closeAction(be, "s3-operation-backend", be.closeNative)
	}
	return be.closeNative()
}

func (be *s3OperationBackend) closeNative() error {
	be.mu.Lock()
	be.sealed = true
	be.mu.Unlock()
	err := be.s3Backend.Close()
	be.active.Wait()
	return err
}

func (be *s3OperationBackend) Save(ctx context.Context, handle backend.Handle, reader backend.RewindReader) error {
	done, err := be.enterTracked()
	if err != nil {
		return err
	}
	defer done()
	if be.cleanup {
		return errS3Denied
	}
	if _, err := be.filename(handle); err != nil {
		return err
	}
	if reader == nil || reader.Length() < 0 || reader.Length() > maxHTTPObjectBytes {
		return errResourceLimit
	}
	if ctx == nil {
		return errS3Config
	}
	if err := ctx.Err(); err != nil {
		return err
	}
	lockID := ""
	if handle.Type == backend.LockFile {
		lockID = handle.Name
	}
	if err := be.owner.noteSave(lockID); err != nil {
		return err
	}
	be.owner.mu.Lock()
	if handle.Type == backend.SnapshotFile {
		be.owner.knownSnapshotID = handle.Name
	}
	be.owner.mu.Unlock()
	succeeded := false
	defer func() {
		if !succeeded {
			be.owner.saveReturned(errS3IO)
			return
		}
	}()
	if handle.Type == backend.KeyFile {
		body, err := io.ReadAll(reader)
		if err != nil {
			return err
		}
		defer zeroBytes(body)
		if err := reader.Rewind(); err != nil {
			return err
		}
		if err := be.owner.recordKeyPayload(handle.Name, body); err != nil {
			return err
		}
	}
	err = be.s3Backend.Save(ctx, handle, reader)
	succeeded = err == nil
	// Outcome bookkeeping remains inside the tracked wrapper on failed writes,
	// too. Native drain must include that work before releasing its owner.
	if be.afterSave != nil {
		be.afterSave()
	}
	return err
}

func (be *s3OperationBackend) Remove(ctx context.Context, handle backend.Handle) error {
	done, err := be.enterTracked()
	if err != nil {
		return err
	}
	defer done()
	if handle.Type == backend.LockFile {
		if !validS3ID(handle.Name) || !be.owner.ownsLock(handle.Name) {
			return errS3Denied
		}
	} else if (isTeardownScope(ctx) || isRetentionScope(ctx)) && (handle.Type == backend.SnapshotFile || handle.Type == backend.PackFile || handle.Type == backend.IndexFile) {
		if !validS3ID(handle.Name) {
			return errS3Denied
		}
	} else {
		return errS3Denied
	}
	err = be.s3Backend.Remove(ctx, handle)
	if handle.Type == backend.LockFile {
		be.owner.lockRemoved(handle.Name, err)
	} else if err != nil {
		be.owner.saveReturned(err)
	}
	return err
}

func (owner *s3Owner) drainPrimary() error {
	if owner.owned.failed() {
		return errOwnedCleanup
	}
	owner.mu.Lock()
	owner.sealing = true
	backends := append([]*s3OperationBackend(nil), owner.backends...)
	owner.mu.Unlock()
	var result error
	for _, be := range backends {
		result = errors.Join(result, be.Close())
		if owner.owned.failed() {
			return errOwnedCleanup
		}
	}
	if result == nil {
		owner.mu.Lock()
		owner.primaryDrained = true
		owner.mu.Unlock()
	}
	return result
}

func (owner *s3Owner) unacknowledgedLocks() []string {
	owner.mu.Lock()
	defer owner.mu.Unlock()
	var result []string
	if owner.record.Active != nil {
		for _, id := range owner.record.Active.Locks {
			if !owner.lockAcks[id] {
				result = append(result, id)
			}
		}
	}
	return result
}
