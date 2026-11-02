// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"

	"github.com/klauspost/compress/zstd"
	"github.com/restic/restic/internal/backend"
	"github.com/restic/restic/internal/crypto"
	"github.com/restic/restic/internal/repository"
)

const (
	maxKeyBytes           = 256 * 1024
	maxSmallMetadataBytes = 64 * 1024
	maxSnapshotBytes      = 16 * 1024 * 1024
	maxIndexBytes         = 8 * 1024 * 1024
	maxPackListCount      = 65536
	// Allow incremental indexes up to the snapshot census, independently of
	// stored/decoded byte budgets and the fixed two-loader memory bound.
	maxIndexListCount    = maxSnapshotCount
	maxIndexListBytes    = 64 * 1024 * 1024
	maxSnapshotListBytes = 256 * 1024 * 1024
	maxLockListCount     = 64
	maxLockListBytes     = 4 * 1024 * 1024
	maxKDFMemory         = 128 * 1024 * 1024
	maxKDFWork           = 1 << 24
)

var errResourceLimit = errors.New("repository resource limit exceeded")

// Unauthenticated key parameters are checked before every upstream key read,
// including a second read through SearchKey. Existing parameters are never reduced.
func validateKeyResources(data []byte) error {
	var key struct {
		KDF        string
		N, R, P    int
		Salt, Data []byte
	}
	if err := json.Unmarshal(data, &key); err != nil {
		return err
	}
	if key.KDF != "scrypt" || key.N <= 1 || key.N&(key.N-1) != 0 || key.R <= 0 || key.P <= 0 || len(key.Salt) != 64 || len(key.Data) < 32 {
		return errors.New("invalid repository key")
	}
	n, r, p := uint64(key.N), uint64(key.R), uint64(key.P)
	// Division checks avoid overflow before multiplying attacker-controlled values.
	// scrypt allocates v=128*N*r, b=128*r*p, and xy=256*r bytes.
	// Bound their sum before multiplication, including the parallelism allocation.
	if r > maxKDFMemory/128 {
		return errResourceLimit
	}
	units := uint64(maxKDFMemory) / (128 * r)
	if n > units || p > units-n || units-n-p < 2 {
		return errResourceLimit
	}
	if r > maxKDFWork || n > maxKDFWork/r || p > maxKDFWork/(n*r) {
		return errResourceLimit
	}
	return nil
}

type guardedBackend struct{ backend.Backend }

func repositoryListBudget(kind backend.FileType) (int, int64) {
	switch kind {
	case backend.KeyFile:
		return maxKeyFiles, maxKeyFiles * maxKeyBytes
	case backend.ConfigFile:
		return 1, maxSmallMetadataBytes
	case backend.LockFile:
		return maxLockListCount, maxLockListBytes
	case backend.SnapshotFile:
		return maxSnapshotCount + 1, maxSnapshotListBytes
	case backend.IndexFile:
		return maxIndexListCount, maxIndexListBytes
	default:
		return maxPackListCount, 0
	}
}

// List bounds repository enumeration before upstream collectors can allocate
// from attacker-controlled object counts or aggregate metadata sizes.
func (guard guardedBackend) List(ctx context.Context, kind backend.FileType, consume func(backend.FileInfo) error) error {
	maxCount, maxBytes := repositoryListBudget(kind)
	count, totalBytes := 0, int64(0)
	return guard.Backend.List(ctx, kind, func(info backend.FileInfo) error {
		count++
		if count > maxCount || info.Size < 0 {
			return errResourceLimit
		}
		if maxBytes > 0 {
			if info.Size > maxBytes-totalBytes {
				return errResourceLimit
			}
			totalBytes += info.Size
		}
		return consume(info)
	})
}

// Bound unpacked metadata before Repository.LoadRaw copies it. Packed file data
// remains streamed through upstream readers; this guard does not replace encryption.
func (guard guardedBackend) Load(ctx context.Context, handle backend.Handle, length int, offset int64, consume func(io.Reader) error) error {
	limit := 0
	switch handle.Type {
	case backend.KeyFile:
		limit = maxKeyBytes
	case backend.ConfigFile, backend.LockFile:
		limit = maxSmallMetadataBytes
	case backend.SnapshotFile:
		limit = maxSnapshotBytes
	case backend.IndexFile:
		limit = maxIndexBytes
	default:
		return guard.Backend.Load(ctx, handle, length, offset, consume)
	}
	if offset != 0 || length < 0 || length > limit {
		return errResourceLimit
	}
	return guard.Backend.Load(ctx, handle, length, offset, func(reader io.Reader) error {
		data, err := io.ReadAll(io.LimitReader(reader, int64(limit)+1))
		if err != nil {
			return err
		}
		if len(data) > limit {
			return errResourceLimit
		}
		if handle.Type == backend.KeyFile {
			if err := validateKeyResources(data); err != nil {
				return err
			}
		} else if len(data) < crypto.Extension {
			// Upstream decryptors slice the nonce before authenticating. Refuse
			// truncated ciphertext here, including reads in upstream goroutines.
			return errors.New("truncated encrypted repository metadata")
		}
		return consume(bytes.NewReader(data))
	})
}

func refusalResult(err error, diagnostic string) bridgeResult {
	if errors.Is(err, errResourceLimit) || errors.Is(err, zstd.ErrDecoderSizeExceeded) || errors.Is(err, repository.ErrMobileIndexBudgetExceeded) {
		return bridgeResult{status: StatusCapExceeded, diagnostic: DiagCapExceeded}
	}
	return bridgeResult{status: StatusFailed, diagnostic: diagnostic}
}
