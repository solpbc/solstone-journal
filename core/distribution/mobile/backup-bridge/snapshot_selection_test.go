// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"strings"
	"testing"
)

func TestSnapshotSelectionRefusesAmbiguousOrUnboundedPolicy(t *testing.T) {
	for _, input := range [][]byte{
		nil, []byte(`null`), []byte(`{}`), []byte(`[] []`), []byte(`[1]`), []byte(`[null]`),
		[]byte(`[""]`), []byte(`["/cache"]`), []byte(`["../secret"]`), []byte(`["a/../secret"]`),
		[]byte(`["a/./b"]`), []byte(`["a//b"]`), []byte(`["!cache"]`), []byte(`["["]`),
		[]byte(`["a\nsecret"]`), []byte(`["a\u0000secret"]`), []byte(`["\ud800"]`),
		[]byte(`["\udc00"]`), []byte(`["\ud800\u0061"]`), {'[', '"', 0xff, '"', ']'},
		[]byte(`["**/a/**/private"]`),
	} {
		if _, status := parseSnapshotSelection(input); status != StatusInvalidArgument {
			t.Fatalf("accepted malformed policy %q: %d", input, status)
		}
	}
	if _, status := parseSnapshotSelection(bytes.Repeat([]byte{' '}, maxSnapshotSelectionBytes+1)); status != StatusCapExceeded {
		t.Fatal("policy byte cap not enforced")
	}
	for _, patterns := range [][]string{
		{strings.Repeat("a", maxPathLen+1)},
		strings.Fields(strings.Repeat("a ", maxSnapshotExclusions+1)),
		{strings.Repeat("a/", maxSnapshotPatternDepth) + "private"},
	} {
		input, _ := json.Marshal(patterns)
		if _, status := parseSnapshotSelection(input); status != StatusCapExceeded {
			t.Fatal("pattern length/count cap not enforced", status)
		}
	}
	for _, input := range []string{`[]`, `["\ud83e\ude81"]`, `["�"]`, `["\\ud800"]`} {
		if _, status := parseSnapshotSelection([]byte(input)); status != StatusOk {
			t.Fatal("valid literal/paired Unicode policy refused", input, status)
		}
	}
}

func TestSnapshotSelectionUsesExactHeldSourceCoordinates(t *testing.T) {
	selection, status := parseSnapshotSelection([]byte(`["cache","config/*-sign-in.json*","**/.removing_*","literal$VAR","🪁"]`))
	if status != StatusOk {
		t.Fatal("fixture policy refused", status)
	}
	root := newRootFS(nil, `journal[*]?\$界`, nil)
	for relative, want := range map[string]bool{
		"": true, "cache": false, "cache/secret": false,
		"owner/cache/notes": true, "owner/config/chatgpt-sign-in.json": true,
		"owner/config/plaud-sign-in.json": true,
		"config/chatgpt-sign-in.json": false, "config/chatgpt-sign-in.json.tmp": false,
		"config/chatgpt-sign-in.json-owner-notes": false,
		"config/plaud-sign-in.json": false, "config/plaud-sign-in.json.lock": false,
		"config/plaud-sign-in.json.renewal": false, "config/plaud-sign-in.json.unreadable-1": false,
		"config/sign-in.json":                     true, "health/durable.json": true,
		"a/.removing_owned/file": false, "a/removing_owner.txt": true,
		"literal$VAR": false, "literal": true, "🪁": false, "🪁-notes": true,
	} {
		item := root.sourcePath
		if relative != "" {
			item += "/" + relative
		}
		if got := selection.permits(context.Background(), root, item); got != want {
			t.Fatalf("selection changed source scope: %q got %v want %v", relative, got, want)
		}
	}
	if selection.failure() != nil {
		t.Fatal("valid source filtering failed")
	}
	if selection.permits(context.Background(), root, "foreign/owner.txt") || selection.failure() == nil {
		t.Fatal("source coordinate error was silently accepted")
	}
}

func TestSnapshotSelectionObservesActualOperationCancellation(t *testing.T) {
	selection, status := parseSnapshotSelection([]byte(`["**/private"]`))
	if status != StatusOk {
		t.Fatal(status)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if selection.permits(ctx, newRootFS(nil, "source", nil), "source/owner.txt") || !errors.Is(selection.failure(), context.Canceled) {
		t.Fatal("cancelled operation admitted source traversal")
	}
}
