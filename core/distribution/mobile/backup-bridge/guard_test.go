// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"testing"

	"github.com/klauspost/compress/zstd"
	"github.com/restic/restic/internal/backend"
	"github.com/restic/restic/internal/repository"
)

type readerBackend struct {
	backend.Backend
	reader func() io.Reader
}

type listBackend struct {
	backend.Backend
	files []backend.FileInfo
}

func (be listBackend) List(_ context.Context, _ backend.FileType, consume func(backend.FileInfo) error) error {
	for _, file := range be.files {
		if err := consume(file); err != nil {
			return err
		}
	}
	return nil
}

func (be readerBackend) Load(_ context.Context, _ backend.Handle, _ int, _ int64, consume func(io.Reader) error) error {
	return consume(be.reader())
}

type endlessReader struct{ read int }

func (reader *endlessReader) Read(buffer []byte) (int, error) {
	for i := range buffer {
		buffer[i] = 'x'
	}
	reader.read += len(buffer)
	return len(buffer), nil
}

func TestOversizedKeyStopsBeforeUpstreamAllocation(t *testing.T) {
	reader := &endlessReader{}
	guard := guardedBackend{readerBackend{reader: func() io.Reader { return reader }}}
	consumed := false
	err := guard.Load(context.Background(), backend.Handle{Type: backend.KeyFile}, 0, 0, func(io.Reader) error { consumed = true; return nil })
	if !errors.Is(err, errResourceLimit) || consumed || reader.read != maxKeyBytes+1 {
		t.Fatalf("unbounded key reached upstream: err=%v bytes=%d consumed=%t", err, reader.read, consumed)
	}
}

func TestEveryKeyReadRechecksParameters(t *testing.T) {
	key := map[string]any{"kdf": "scrypt", "N": 32768, "r": 8, "p": 1, "salt": make([]byte, 64), "data": make([]byte, 64)}
	guard := guardedBackend{readerBackend{reader: func() io.Reader { data, _ := json.Marshal(key); return bytes.NewReader(data) }}}
	reads := 0
	consume := func(io.Reader) error { reads++; return nil }
	if err := guard.Load(context.Background(), backend.Handle{Type: backend.KeyFile}, 0, 0, consume); err != nil {
		t.Fatal(err)
	}
	key["N"] = 1 << 40
	if err := guard.Load(context.Background(), backend.Handle{Type: backend.KeyFile}, 0, 0, consume); !errors.Is(err, errResourceLimit) {
		t.Fatalf("changed parameters accepted: %v", err)
	}
	if reads != 1 {
		t.Fatal("second unsafe key reached upstream")
	}
}

func TestTruncatedEncryptedMetadataNeverReachesDecryptor(t *testing.T) {
	for _, kind := range []backend.FileType{backend.ConfigFile, backend.LockFile, backend.SnapshotFile, backend.IndexFile} {
		for size := 0; size < 32; size++ {
			guard := guardedBackend{readerBackend{reader: func() io.Reader { return bytes.NewReader(make([]byte, size)) }}}
			consumed := false
			err := guard.Load(context.Background(), backend.Handle{Type: kind}, 0, 0, func(io.Reader) error { consumed = true; return nil })
			if err == nil || consumed {
				t.Fatalf("truncated %v ciphertext reached upstream: bytes=%d", kind, size)
			}
		}
	}
}

func TestRefusalResultDecoderSizeExceeded(t *testing.T) {
	for _, err := range []error{zstd.ErrDecoderSizeExceeded, repository.ErrMobileIndexBudgetExceeded} {
		result := refusalResult(errors.Join(errors.New("decode index"), err), DiagRepoCorrupt)
		if result.status != StatusCapExceeded || result.diagnostic != DiagCapExceeded {
			t.Fatalf("decoder cap must be reported as cap refusal, got status=%d diagnostic=%q", result.status, result.diagnostic)
		}
	}
}

func TestRepositoryObjectEnumerationIsBounded(t *testing.T) {
	files := make([]backend.FileInfo, maxIndexListCount+1)
	guard := guardedBackend{listBackend{files: files}}
	seen := 0
	err := guard.List(context.Background(), backend.IndexFile, func(backend.FileInfo) error {
		seen++
		return nil
	})
	if !errors.Is(err, errResourceLimit) || seen != maxIndexListCount {
		t.Fatalf("unbounded list reached upstream: seen=%d err=%v", seen, err)
	}
}

func TestRepositoryMetadataEnumerationHasAggregateByteBound(t *testing.T) {
	files := []backend.FileInfo{{Size: maxIndexListBytes}, {Size: 1}}
	guard := guardedBackend{listBackend{files: files}}
	seen := 0
	err := guard.List(context.Background(), backend.IndexFile, func(backend.FileInfo) error {
		seen++
		return nil
	})
	if !errors.Is(err, errResourceLimit) || seen != 1 {
		t.Fatalf("aggregate metadata cap failed: seen=%d err=%v", seen, err)
	}
}
