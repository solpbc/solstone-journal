// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

import (
	"bytes"
	"crypto/rand"
	"crypto/subtle"
	"encoding/hex"
	"encoding/json"
	"errors"
	"io"
	"net"
	"net/netip"
	"net/url"
	"os"
	"strconv"
	"strings"
	"sync"

	"golang.org/x/sys/unix"
)

const (
	s3OwnerStateName      = "state.json"
	s3OwnerTempPrefix     = ".s3-state-"
	maxS3OwnerBytes       = 128 * 1024
	maxS3OwnerCredentials = 4
	maxS3OwnerPasswords   = 16
	maxS3OwnerLocks       = 32
)

var (
	errS3OwnerInvalid = errors.New("S3_OWNER_STATE_INVALID")
	errS3OwnerIO      = errors.New("S3_OWNER_STATE_IO_FAILED")
	errS3OwnerPending = errors.New("S3_REMOTE_PENDING")
	errS3OwnerBusy    = errors.New("S3_DESTINATION_BUSY")
	errS3OwnerPolicy  = errors.New("S3_OWNER_FILE_POLICY_REFUSED")
	errS3RecoveryHold = errors.New("S3_RECOVERY_HOLD")
	s3OwnersMu        sync.Mutex
	s3Owners          = make(map[s3Destination]*s3Owner)
)

type s3Destination struct {
	Endpoint string `json:"endpoint"`
	Region   string `json:"region"`
	Bucket   string `json:"bucket"`
	Prefix   string `json:"prefix"`
}

type s3RetainedCredential struct {
	Access string `json:"access"`
	Secret string `json:"secret"`
	Token  string `json:"token"`
}

type s3OwnerIntent struct {
	ID               string   `json:"id"`
	Operation        int32    `json:"operation"`
	MayHaveWritten   bool     `json:"may_have_written"`
	Locks            []string `json:"locks"`
	RecoveryPassword []byte   `json:"recovery_password,omitempty"`
	KeyID            string   `json:"key_id,omitempty"`
	Ciphertext       []byte   `json:"ciphertext,omitempty"`
	RotationCustody  bool     `json:"rotation_custody,omitempty"`
	TeardownScope    string   `json:"teardown_scope,omitempty"`
	TeardownOrigin   string   `json:"teardown_origin,omitempty"`
}

type s3RecoveryHold struct {
	KeyID            string `json:"key_id"`
	RecoveryPassword []byte `json:"recovery_password"`
	Ciphertext       []byte `json:"ciphertext"`
}

// A completed record is prepared only after observed write/lock outcomes and
// actual consequential work drain. Its visibility is not server quiescence.
type s3OwnerCompletion struct {
	ID                 string `json:"id"`
	Operation          int32  `json:"operation"`
	TeardownScope      string `json:"teardown_scope,omitempty"`
	TeardownOrigin     string `json:"teardown_origin,omitempty"`
	TeardownReconciled bool   `json:"teardown_reconciled,omitempty"`
}

type s3OwnerRecord struct {
	Version      int                    `json:"version"`
	Destination  s3Destination          `json:"destination"`
	Credentials  []s3RetainedCredential `json:"credentials"`
	Passwords    [][]byte               `json:"passwords"`
	Active       *s3OwnerIntent         `json:"active"`
	Complete     *s3OwnerCompletion     `json:"complete"`
	RecoveryHold *s3RecoveryHold        `json:"recovery_hold"`
}

type s3Owner struct {
	owned              *ownedCall
	mu                 sync.Mutex
	path               string
	destination        s3Destination
	directory          *os.File
	root               *os.Root
	stateFile          *os.File
	record             s3OwnerRecord
	exists             bool
	closed             bool
	sealing            bool
	begun              bool
	primaryDrained     bool
	liveID             string
	liveCredential     s3RetainedCredential
	baselineCreds      []s3RetainedCredential
	baselinePasses     [][]byte
	attemptFailed      bool
	teardownReconciled bool
	cleanupActive      bool
	attemptPasses      [][]byte
	knownKeyID         string
	knownSnapshotID    string
	writeFailed        bool
	stateFailed        bool
	lockAcks           map[string]bool
	backends           []*s3OperationBackend
	// Private deterministic fault/barrier injection; no public setter or ABI.
	fault                 func(string) error
	preservedAddIntent    *s3OwnerIntent
	preservedRemoveIntent *s3OwnerIntent
}

func s3DestinationFor(cfg s3Config) (s3Destination, error) {
	if cfg.validate() != nil {
		return s3Destination{}, errS3Config
	}
	parsed, _ := url.Parse(cfg.endpoint)
	port := parsed.Port()
	if port == "" {
		port = "443"
	}
	number, _ := strconv.ParseUint(port, 10, 16)
	host := strings.ToLower(parsed.Hostname())
	if address, err := netip.ParseAddr(host); err == nil {
		host = address.String()
	}
	endpoint := "https://" + net.JoinHostPort(host, strconv.FormatUint(number, 10))
	return s3Destination{endpoint, cfg.region, cfg.bucket, cfg.prefix}, nil
}

func (destination s3Destination) config(credential s3RetainedCredential) s3Config {
	return s3Config{endpoint: destination.Endpoint, region: destination.Region, bucket: destination.Bucket,
		prefix: destination.Prefix, access: credential.Access, secret: credential.Secret, token: credential.Token}
}

func newS3Owner(path string, cfg s3Config) (_ *s3Owner, resultErr error) {
	return newS3OwnerOwned(path, cfg, nil)
}

func newS3OwnerOwned(path string, cfg s3Config, entry *ownedCall) (_ *s3Owner, resultErr error) {
	return newS3OwnerExpected(path, cfg, entry, nil)
}

func newS3OwnerExpected(path string, cfg s3Config, entry *ownedCall, expected *borrowedDirectoryIdentity) (_ *s3Owner, resultErr error) {
	destination, err := s3DestinationFor(cfg)
	if err != nil || sanitizePath(path) != nil {
		return nil, errS3OwnerInvalid
	}
	owner := &s3Owner{owned: entry, path: path, destination: destination, lockAcks: make(map[string]bool)}
	entry.retainOwner(owner)
	s3OwnersMu.Lock()
	if s3Owners[destination] != nil {
		s3OwnersMu.Unlock()
		return nil, errS3OwnerBusy
	}
	s3Owners[destination] = owner
	s3OwnersMu.Unlock()
	defer func() {
		if resultErr != nil {
			entry.scrubRejection()
			_ = owner.Close()
		}
	}()
	file, err := openOwnedDirectory(path, entry)
	if err != nil {
		return nil, errS3OwnerInvalid
	}
	owner.directory = file
	var stat unix.Stat_t
	if unix.Fstat(int(file.Fd()), &stat) != nil || stat.Uid != uint32(os.Geteuid()) || stat.Mode&07777 != 0700 {
		return nil, errS3OwnerInvalid
	}
	if !matchesBorrowedDirectory("state", file, expected) {
		return nil, errS3OwnerInvalid
	}
	if err := reserveOwnedPrivateDirectory(file, entry); err != nil {
		return nil, err
	}
	if err := unix.Flock(int(file.Fd()), unix.LOCK_EX|unix.LOCK_NB); err != nil {
		if errors.Is(err, unix.EWOULDBLOCK) || errors.Is(err, unix.EAGAIN) {
			return nil, errS3OwnerBusy
		}
		return nil, errS3OwnerIO
	}
	if err := s3StateRootPolicy(int(file.Fd())); err != nil {
		return nil, errS3OwnerPolicy
	}
	owner.root, err = adoptOwnedRoot(path, file, entry)
	if err != nil {
		return nil, errS3OwnerInvalid
	}
	if err := owner.census(""); err != nil {
		return nil, err
	}
	if err := owner.read(); err != nil {
		return nil, err
	}
	return owner, nil
}

func (owner *s3Owner) gate(stage string) error {
	if owner.fault != nil {
		return owner.fault(stage)
	}
	return nil
}

// No residue is deleted here. An interrupted private temp is explicit custody,
// rather than permission to reset a record or discard credential material.
func (owner *s3Owner) census(temporary string) error {
	fd, err := unix.Openat(int(owner.directory.Fd()), ".", unix.O_RDONLY|unix.O_DIRECTORY|unix.O_CLOEXEC, 0)
	if err != nil {
		return errS3OwnerIO
	}
	listing := os.NewFile(uintptr(fd), "S3 owner state census")
	names, readErr := listing.Readdirnames(10)
	if owner.owned != nil {
		invalid := readErr != nil && !errors.Is(readErr, io.EOF) || len(names) > 2
		for _, name := range names {
			invalid = invalid || name != s3OwnerStateName && (temporary == "" || name != temporary)
		}
		if invalid {
			owner.owned.scrubRejection()
		}
	}
	closeErr := owner.owned.close(listing, "s3-state-census")
	if readErr != nil && !errors.Is(readErr, io.EOF) || closeErr != nil || len(names) > 2 {
		return errS3OwnerInvalid
	}
	for _, name := range names {
		if name != s3OwnerStateName && (temporary == "" || name != temporary) {
			return errS3OwnerInvalid
		}
	}
	return nil
}

func privateS3StateFile(file *os.File) bool {
	var stat unix.Stat_t
	return file != nil && unix.Fstat(int(file.Fd()), &stat) == nil && stat.Mode&unix.S_IFMT == unix.S_IFREG &&
		stat.Mode&07777 == 0600 && stat.Uid == uint32(os.Geteuid()) && stat.Nlink == 1 && stat.Size >= 0 && stat.Size <= maxS3OwnerBytes
}

func (owner *s3Owner) read() error {
	fd, err := unix.Openat(int(owner.directory.Fd()), s3OwnerStateName, unix.O_RDONLY|unix.O_NOFOLLOW|unix.O_CLOEXEC|unix.O_NONBLOCK, 0)
	if errors.Is(err, unix.ENOENT) {
		return nil
	}
	if err != nil {
		return errS3OwnerInvalid
	}
	file := os.NewFile(uintptr(fd), "S3 owner state")
	keep := false
	defer func() {
		if !keep {
			owner.owned.scrubRejection()
			_ = owner.owned.close(file, "s3-state-rejected-read")
		}
	}()
	if !privateS3StateFile(file) || s3StateFilePolicy(fd, false) != nil {
		return errS3OwnerInvalid
	}
	raw, err := io.ReadAll(io.LimitReader(file, maxS3OwnerBytes+1))
	defer zeroBytes(raw)
	if err != nil || len(raw) > maxS3OwnerBytes || uniqueS3StateJSON(raw) != nil || completeS3StateJSON(raw) != nil {
		return errS3OwnerInvalid
	}
	var record s3OwnerRecord
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.DisallowUnknownFields()
	if decoder.Decode(&record) != nil || record.validate() != nil || record.Destination != owner.destination {
		return errS3OwnerInvalid
	}
	if matches, err := pinnedEntryMatches(int(owner.directory.Fd()), s3OwnerStateName, file); err != nil || !matches {
		return errS3OwnerInvalid
	}
	owner.stateFile, owner.record, owner.exists = file, record, true
	keep = true
	return nil
}

func uniqueS3StateJSON(raw []byte) error {
	decoder := json.NewDecoder(bytes.NewReader(raw))
	var value func(int) error
	value = func(depth int) error {
		if depth > 12 {
			return errS3OwnerInvalid
		}
		token, err := decoder.Token()
		if err != nil {
			return errS3OwnerInvalid
		}
		delimiter, nested := token.(json.Delim)
		if !nested {
			return nil
		}
		switch delimiter {
		case '{':
			seen := make(map[string]bool)
			for decoder.More() {
				key, err := decoder.Token()
				if err != nil {
					return errS3OwnerInvalid
				}
				name, ok := key.(string)
				if !ok || seen[name] || len(seen) >= 32 {
					return errS3OwnerInvalid
				}
				seen[name] = true
				if err := value(depth + 1); err != nil {
					return err
				}
			}
			end, err := decoder.Token()
			if err != nil || end != json.Delim('}') {
				return errS3OwnerInvalid
			}
		case '[':
			count := 0
			for decoder.More() {
				count++
				if count > 32 {
					return errS3OwnerInvalid
				}
				if err := value(depth + 1); err != nil {
					return err
				}
			}
			end, err := decoder.Token()
			if err != nil || end != json.Delim(']') {
				return errS3OwnerInvalid
			}
		default:
			return errS3OwnerInvalid
		}
		return nil
	}
	if err := value(0); err != nil {
		return err
	}
	if _, err := decoder.Token(); !errors.Is(err, io.EOF) {
		return errS3OwnerInvalid
	}
	return nil
}

// Missing/null booleans cannot default an interrupted write to "no egress".
// Exact field presence complements duplicate rejection and typed validation.
func completeS3StateJSON(raw []byte) error {
	object := func(raw []byte, names []string, nullable map[string]bool) (map[string]json.RawMessage, error) {
		var fields map[string]json.RawMessage
		if json.Unmarshal(raw, &fields) != nil || len(fields) != len(names) {
			return nil, errS3OwnerInvalid
		}
		for _, name := range names {
			value, present := fields[name]
			if !present || (bytes.Equal(bytes.TrimSpace(value), []byte("null")) && !nullable[name]) {
				return nil, errS3OwnerInvalid
			}
		}
		return fields, nil
	}
	var fields map[string]json.RawMessage
	if json.Unmarshal(raw, &fields) != nil {
		return errS3OwnerInvalid
	}
	if len(fields) != 6 && len(fields) != 7 {
		return errS3OwnerInvalid
	}
	required6 := []string{"version", "destination", "credentials", "passwords", "active", "complete"}
	nullable := map[string]bool{"active": true, "complete": true, "recovery_hold": true}
	for _, name := range required6 {
		value, present := fields[name]
		if !present || (bytes.Equal(bytes.TrimSpace(value), []byte("null")) && !nullable[name]) {
			return errS3OwnerInvalid
		}
	}
	if len(fields) == 7 {
		value, present := fields["recovery_hold"]
		if !present || (bytes.Equal(bytes.TrimSpace(value), []byte("null")) && !nullable["recovery_hold"]) {
			return errS3OwnerInvalid
		}
	}
	if _, err := object(fields["destination"], []string{"endpoint", "region", "bucket", "prefix"}, nil); err != nil {
		return err
	}
	var credentials []json.RawMessage
	if json.Unmarshal(fields["credentials"], &credentials) != nil || len(credentials) > maxS3OwnerCredentials {
		return errS3OwnerInvalid
	}
	for _, credential := range credentials {
		if _, err := object(credential, []string{"access", "secret", "token"}, nil); err != nil {
			return err
		}
	}
	if !bytes.Equal(bytes.TrimSpace(fields["active"]), []byte("null")) {
		var activeFields map[string]json.RawMessage
		if json.Unmarshal(fields["active"], &activeFields) != nil {
			return errS3OwnerInvalid
		}
		for name, value := range activeFields {
			switch name {
			case "id", "operation", "may_have_written":
				if bytes.Equal(bytes.TrimSpace(value), []byte("null")) {
					return errS3OwnerInvalid
				}
			case "locks":
				// nullable
			case "recovery_password", "key_id", "ciphertext", "teardown_scope", "teardown_origin":
				if bytes.Equal(bytes.TrimSpace(value), []byte("null")) {
					return errS3OwnerInvalid
				}
			case "rotation_custody":
				if bytes.Equal(bytes.TrimSpace(value), []byte("null")) {
					return errS3OwnerInvalid
				}
				var b bool
				if json.Unmarshal(value, &b) != nil {
					return errS3OwnerInvalid
				}
			default:
				return errS3OwnerInvalid
			}
		}
		for _, reqName := range []string{"id", "operation", "may_have_written", "locks"} {
			if _, present := activeFields[reqName]; !present {
				return errS3OwnerInvalid
			}
		}
	}
	if !bytes.Equal(bytes.TrimSpace(fields["complete"]), []byte("null")) {
		var complete map[string]json.RawMessage
		if json.Unmarshal(fields["complete"], &complete) != nil {
			return errS3OwnerInvalid
		}
		names := []string{"id", "operation"}
		if len(complete) == 4 || len(complete) == 5 {
			names = append(names, "teardown_scope", "teardown_origin")
		}
		if len(complete) == 5 {
			names = append(names, "teardown_reconciled")
			var reconciled bool
			if json.Unmarshal(complete["teardown_reconciled"], &reconciled) != nil {
				return errS3OwnerInvalid
			}
		}
		if _, err := object(fields["complete"], names, nil); err != nil {
			return err
		}
	}
	if holdRaw, present := fields["recovery_hold"]; present && !bytes.Equal(bytes.TrimSpace(holdRaw), []byte("null")) {
		if _, err := object(holdRaw, []string{"key_id", "recovery_password", "ciphertext"}, nil); err != nil {
			return err
		}
	}
	return nil
}

func validOwnerOperation(operation int32) bool {
	switch operation {
	case OpInit, OpOpen, OpAddRecoveryKey, OpSnapshot, OpListSnapshots, OpRestore, OpCheck, OpListRecoveryKeys, OpRemoveRecoveryKey, OpTeardown, OpRetention, OpArchiveSources, OpListNodes, OpReadFile:
		return true
	default:
		return false
	}
}

func (record s3OwnerRecord) validate() error {
	if record.Version != 1 || len(record.Credentials) == 0 || len(record.Credentials) > maxS3OwnerCredentials || len(record.Passwords) == 0 || len(record.Passwords) > maxS3OwnerPasswords || (record.Active == nil) == (record.Complete == nil) {
		return errS3OwnerInvalid
	}
	for i, credential := range record.Credentials {
		cfg := record.Destination.config(credential)
		destination, err := s3DestinationFor(cfg)
		if err != nil || destination != record.Destination {
			return errS3OwnerInvalid
		}
		for _, previous := range record.Credentials[:i] {
			if previous == credential {
				return errS3OwnerInvalid
			}
		}
	}
	for i, password := range record.Passwords {
		if len(password) == 0 || len(password) > maxPasswordLen {
			return errS3OwnerInvalid
		}
		for _, previous := range record.Passwords[:i] {
			if bytes.Equal(previous, password) {
				return errS3OwnerInvalid
			}
		}
	}
	if active := record.Active; active != nil {
		if (active.TeardownScope != "" || active.TeardownOrigin != "") && (active.Operation != OpTeardown || !teardownHex(active.TeardownScope, 64) || !isMetadataNonce(active.TeardownOrigin)) {
			return errS3OwnerInvalid
		}
		if !isMetadataNonce(active.ID) || !validOwnerOperation(active.Operation) || len(active.Locks) > maxS3OwnerLocks || len(active.Locks) != 0 && !active.MayHaveWritten {
			return errS3OwnerInvalid
		}
		if active.KeyID != "" && (!lowerHexID(active.KeyID) || len(active.KeyID) != 64) {
			return errS3OwnerInvalid
		}
		if active.Operation == OpAddRecoveryKey && active.RotationCustody && active.MayHaveWritten && len(active.RecoveryPassword) == 0 {
			return errS3OwnerInvalid
		}
		if len(active.RecoveryPassword) > maxPasswordLen || (active.Operation != OpAddRecoveryKey && len(active.RecoveryPassword) != 0) {
			return errS3OwnerInvalid
		}
		if len(active.Ciphertext) > 0 && (active.Operation != OpAddRecoveryKey || len(active.Ciphertext) > maxS3OwnerBytes) {
			return errS3OwnerInvalid
		}
		seen := make(map[string]bool)
		for _, id := range active.Locks {
			if !validS3ID(id) || seen[id] {
				return errS3OwnerInvalid
			}
			seen[id] = true
		}
	}
	if complete := record.Complete; complete != nil {
		if complete.TeardownReconciled && (complete.Operation != OpTeardown || complete.TeardownScope == "" || complete.TeardownOrigin == "") {
			return errS3OwnerInvalid
		}
		if (complete.TeardownScope != "" || complete.TeardownOrigin != "") && (complete.Operation != OpTeardown || !teardownHex(complete.TeardownScope, 64) || !isMetadataNonce(complete.TeardownOrigin)) {
			return errS3OwnerInvalid
		}
		if !isMetadataNonce(complete.ID) || !validOwnerOperation(complete.Operation) {
			return errS3OwnerInvalid
		}
	}
	if hold := record.RecoveryHold; hold != nil {
		if !lowerHexID(hold.KeyID) || len(hold.KeyID) != 64 {
			return errS3OwnerInvalid
		}
		if len(hold.RecoveryPassword) == 0 || len(hold.RecoveryPassword) > maxPasswordLen {
			return errS3OwnerInvalid
		}
		if len(hold.Ciphertext) == 0 || len(hold.Ciphertext) > maxS3OwnerBytes {
			return errS3OwnerInvalid
		}
	}
	return nil
}

func (owner *s3Owner) recheckDirectory() (resultErr error) {
	file, err := openOwnedDirectory(owner.path, owner.owned)
	if err != nil {
		return errS3OwnerInvalid
	}
	defer func() {
		err := owner.owned.close(file, "s3-state-recheck-directory")
		if owner.owned != nil {
			resultErr = errors.Join(resultErr, err)
		}
	}()
	same, err := sameDirectory(file, owner.directory)
	if err != nil || !same {
		return errS3OwnerInvalid
	}
	var stat unix.Stat_t
	if unix.Fstat(int(owner.directory.Fd()), &stat) != nil || stat.Uid != uint32(os.Geteuid()) || stat.Mode&07777 != 0700 || s3StateRootPolicy(int(owner.directory.Fd())) != nil {
		return errS3OwnerInvalid
	}
	return nil
}

func (owner *s3Owner) recheckCustody(temporary string) error {
	if owner.recheckDirectory() != nil || owner.census(temporary) != nil {
		return errS3OwnerInvalid
	}
	if owner.stateFile != nil {
		if !privateS3StateFile(owner.stateFile) || s3StateFilePolicy(int(owner.stateFile.Fd()), false) != nil {
			return errS3OwnerInvalid
		}
		if matches, err := pinnedEntryMatches(int(owner.directory.Fd()), s3OwnerStateName, owner.stateFile); err != nil || !matches {
			return errS3OwnerInvalid
		}
	} else {
		var stat unix.Stat_t
		if err := unix.Fstatat(int(owner.directory.Fd()), s3OwnerStateName, &stat, unix.AT_SYMLINK_NOFOLLOW); !errors.Is(err, unix.ENOENT) {
			return errS3OwnerInvalid
		}
	}
	return nil
}

// persist reports rename visibility separately from its final sync acknowledgement.
// The caller holds owner.mu. No payload is written before actual file-policy checks.
func (owner *s3Owner) persist(record s3OwnerRecord) (published bool, resultErr error) {
	if record.validate() != nil || owner.recheckCustody("") != nil {
		return false, errS3OwnerInvalid
	}
	encoded, err := json.Marshal(record)
	if err != nil || len(encoded) > maxS3OwnerBytes {
		return false, errResourceLimit
	}
	defer zeroBytes(encoded)
	var nonce [16]byte
	if _, err := rand.Read(nonce[:]); err != nil {
		return false, errS3OwnerIO
	}
	name := s3OwnerTempPrefix + hex.EncodeToString(nonce[:]) + ".tmp"
	fd, err := unix.Openat(int(owner.directory.Fd()), name, unix.O_WRONLY|unix.O_CREAT|unix.O_EXCL|unix.O_NOFOLLOW|unix.O_CLOEXEC, 0600)
	if err != nil {
		return false, errS3OwnerIO
	}
	file := os.NewFile(uintptr(fd), "S3 owner state temporary")
	defer func() { _ = owner.owned.close(file, "s3-state-write-temporary") }()
	if !privateS3StateFile(file) || s3StateFilePolicy(fd, true) != nil {
		return false, errS3OwnerPolicy
	}
	if owner.gate("before-secret-write") != nil {
		return false, errS3OwnerIO
	}
	if n, err := file.Write(encoded); err != nil || n != len(encoded) {
		return false, errS3OwnerIO
	}
	if owner.gate("file-sync") != nil || s3StateSyncFile(fd) != nil {
		return false, errS3OwnerIO
	}
	if owner.gate("after-file-sync") != nil {
		return false, errS3OwnerIO
	}
	readFD, err := unix.Openat(int(owner.directory.Fd()), name, unix.O_RDONLY|unix.O_NOFOLLOW|unix.O_CLOEXEC|unix.O_NONBLOCK, 0)
	if err != nil {
		return false, errS3OwnerIO
	}
	identity := os.NewFile(uintptr(readFD), "S3 owner state identity")
	keepIdentity := false
	defer func() {
		if !keepIdentity {
			_ = owner.owned.close(identity, "s3-state-read-identity")
		}
	}()
	if same, err := sameDirectory(file, identity); err != nil || !same || !privateS3StateFile(identity) {
		return false, errS3OwnerInvalid
	}
	if err := owner.owned.close(file, "s3-state-write-temporary"); err != nil {
		return false, errS3OwnerIO
	}
	if owner.gate("before-rename") != nil || owner.recheckCustody(name) != nil {
		return false, errS3OwnerIO
	}
	if matches, err := pinnedEntryMatches(int(owner.directory.Fd()), name, identity); err != nil || !matches {
		return false, errS3OwnerInvalid
	}
	if err := owner.gate("atomic-replace"); err != nil {
		return false, errS3OwnerIO
	}
	if err := s3StateReplace(int(owner.directory.Fd()), name, s3OwnerStateName, owner.stateFile != nil); err != nil {
		return false, errS3OwnerIO
	}
	if owner.stateFile != nil {
		// Exchange preserves the displaced inode even if a nonconforming same-UID
		// writer changed the target in the final check/syscall interval.
		matches, err := pinnedEntryMatches(int(owner.directory.Fd()), name, owner.stateFile)
		newMatches, newErr := pinnedEntryMatches(int(owner.directory.Fd()), s3OwnerStateName, identity)
		if err != nil || !matches || newErr != nil || !newMatches || !privateS3StateFile(identity) || s3StateFilePolicy(readFD, false) != nil || !privateS3StateFile(owner.stateFile) || s3StateFilePolicy(int(owner.stateFile.Fd()), false) != nil {
			_ = unix.Fsync(int(owner.directory.Fd()))
			return true, errS3OwnerInvalid
		}
		if err := unix.Unlinkat(int(owner.directory.Fd()), name, 0); err != nil {
			_ = unix.Fsync(int(owner.directory.Fd()))
			return true, errS3OwnerIO
		}
		if err := owner.owned.close(owner.stateFile, "s3-state-displaced-file"); err != nil && owner.owned != nil {
			return true, err
		}
	}
	owner.stateFile, owner.record, owner.exists = identity, record, true
	keepIdentity = true
	published = true
	if matches, err := pinnedEntryMatches(int(owner.directory.Fd()), s3OwnerStateName, identity); err != nil || !matches || s3StateFilePolicy(readFD, false) != nil {
		return true, errS3OwnerInvalid
	}
	if owner.gate("directory-sync") != nil || unix.Fsync(int(owner.directory.Fd())) != nil {
		return true, errS3OwnerIO
	}
	return true, nil
}

func (owner *s3Owner) begin(operation int32, rotationCustody bool, cfg s3Config, passwords ...[]byte) error {
	owner.mu.Lock()
	defer owner.mu.Unlock()
	if owner.closed || owner.sealing || !validOwnerOperation(operation) || operation == OpTeardown {
		return errS3OwnerInvalid
	}
	if owner.record.Active != nil && owner.record.Active.Operation == OpTeardown {
		return errS3OwnerPending
	}
	if operation == OpAddRecoveryKey && owner.record.RecoveryHold != nil {
		hold := owner.record.RecoveryHold
		var callerRecPw []byte
		if len(passwords) > 1 {
			callerRecPw = passwords[1]
		}
		if len(callerRecPw) == 0 || len(callerRecPw) != len(hold.RecoveryPassword) ||
			subtle.ConstantTimeCompare(callerRecPw, hold.RecoveryPassword) != 1 {
			return errS3RecoveryHold
		}
	}
	// A prior attempt that may have written stays in custody: this attempt
	// adopts its recorded lock IDs and cannot complete until each is removed.
	// Only attempts whose other remote effects are content-addressed restic
	// objects may be adopted; uncertain key or config writes stay pending.
	inherited := []string{}
	if prior := owner.record.Active; prior != nil && prior.MayHaveWritten {
		if prior.Operation == OpAddRecoveryKey && (operation == OpAddRecoveryKey || operation == OpListRecoveryKeys) {
			inherited = append(inherited, prior.Locks...)
		} else if prior.Operation == OpRemoveRecoveryKey && (operation == OpRemoveRecoveryKey || operation == OpListRecoveryKeys || operation == OpOpen) {
			inherited = append(inherited, prior.Locks...)
		} else if prior.Operation == OpListRecoveryKeys && (operation == OpRemoveRecoveryKey || operation == OpListRecoveryKeys) {
			inherited = append(inherited, prior.Locks...)
		} else if adoptsUncertainty(prior.Operation) && adoptsUncertainty(operation) {
			inherited = append(inherited, prior.Locks...)
		} else {
			return errS3OwnerPending
		}
	}
	destination, err := s3DestinationFor(cfg)
	if err != nil || destination != owner.destination {
		return errS3OwnerInvalid
	}
	if prior := owner.record.Active; prior != nil && prior.MayHaveWritten && prior.Operation == OpAddRecoveryKey {
		if operation == OpListRecoveryKeys {
			owner.begun = true
			owner.liveID = prior.ID
			owner.preservedAddIntent = prior
			return nil
		}
		if operation == OpAddRecoveryKey {
			if len(prior.RecoveryPassword) == 0 {
				return errS3OwnerPending
			}
			owner.begun = true
			owner.liveID = prior.ID
			owner.baselineCreds = append([]s3RetainedCredential(nil), owner.record.Credentials...)
			owner.baselinePasses = append([][]byte(nil), owner.record.Passwords...)
			owner.attemptFailed = false
			credential := s3RetainedCredential{cfg.access, cfg.secret, cfg.token}
			foundCred := false
			for _, retained := range owner.record.Credentials {
				if retained == credential {
					foundCred = true
					break
				}
			}
			if !foundCred {
				owner.record.Credentials = append(owner.record.Credentials, credential)
			}
			owner.liveCredential = credential
			if len(prior.RecoveryPassword) > 0 {
				owner.attemptPasses = [][]byte{append([]byte(nil), passwords[0]...), append([]byte(nil), prior.RecoveryPassword...)}
			} else if len(passwords) > 0 {
				owner.attemptPasses = [][]byte{append([]byte(nil), passwords[0]...)}
			}
			if prior.KeyID != "" {
				owner.knownKeyID = prior.KeyID
			}
			return nil
		}
	}
	if prior := owner.record.Active; prior != nil && prior.MayHaveWritten && prior.Operation == OpRemoveRecoveryKey {
		if operation == OpOpen || operation == OpListRecoveryKeys {
			owner.begun = true
			owner.liveID = prior.ID
			owner.preservedRemoveIntent = prior
			return nil
		}
	}
	record := owner.record
	record.Version, record.Destination = 1, owner.destination
	baselineCreds := append([]s3RetainedCredential(nil), record.Credentials...)
	baselinePasses := append([][]byte(nil), record.Passwords...)
	record.Credentials = append([]s3RetainedCredential(nil), record.Credentials...)
	record.Passwords = append([][]byte(nil), record.Passwords...)
	credential := s3RetainedCredential{cfg.access, cfg.secret, cfg.token}
	found := false
	for _, retained := range record.Credentials {
		if retained == credential {
			found = true
		}
	}
	if !found {
		record.Credentials = append(record.Credentials, credential)
	}
	var attemptPasses [][]byte
	for _, password := range passwords {
		if len(password) == 0 {
			continue
		}
		found = false
		for _, seen := range attemptPasses {
			if bytes.Equal(seen, password) {
				found = true
				break
			}
		}
		if !found {
			attemptPasses = append(attemptPasses, append([]byte(nil), password...))
		}
	}
	owner.attemptPasses = attemptPasses

	for i, password := range passwords {
		if len(password) == 0 {
			continue
		}
		if operation == OpAddRecoveryKey && i > 0 {
			continue
		}
		found = false
		for _, retained := range record.Passwords {
			if bytes.Equal(retained, password) {
				found = true
				break
			}
		}
		if !found {
			record.Passwords = append(record.Passwords, append([]byte(nil), password...))
		}
	}
	var nonce [16]byte
	if _, err := rand.Read(nonce[:]); err != nil {
		return errS3OwnerIO
	}
	record.Active = &s3OwnerIntent{
		ID:              hex.EncodeToString(nonce[:]),
		Operation:       operation,
		MayHaveWritten:  owner.record.Active != nil && owner.record.Active.MayHaveWritten,
		Locks:           inherited,
		RotationCustody: operation == OpAddRecoveryKey && rotationCustody,
	}
	if prior := owner.record.Active; prior != nil && prior.Operation == OpRemoveRecoveryKey && prior.KeyID != "" {
		record.Active.KeyID = prior.KeyID
	}
	record.Complete = nil
	if operation == OpAddRecoveryKey && owner.record.RecoveryHold != nil {
		hold := owner.record.RecoveryHold
		record.Active.KeyID = hold.KeyID
		record.Active.RecoveryPassword = append([]byte(nil), hold.RecoveryPassword...)
		record.Active.Ciphertext = append([]byte(nil), hold.Ciphertext...)
		record.Active.MayHaveWritten = true
		record.Active.RotationCustody = true
		owner.knownKeyID = hold.KeyID
	}
	if len(record.Credentials) > maxS3OwnerCredentials || len(record.Passwords) > maxS3OwnerPasswords {
		return errResourceLimit
	}
	_, err = owner.persist(record)
	owner.begun = true
	if err != nil {
		owner.stateFailed = true
	} else {
		owner.liveID = record.Active.ID
		owner.baselineCreds, owner.baselinePasses, owner.attemptFailed = baselineCreds, baselinePasses, false
		owner.liveCredential = credential
	}
	return err
}

func (owner *s3Owner) beginTeardown(cfg s3Config, admission *s3TeardownAdmission, ids []string, passwords ...[]byte) error {
	owner.mu.Lock()
	defer owner.mu.Unlock()
	if owner.closed || owner.sealing || admission == nil || owner.record.RecoveryHold != nil {
		return errS3OwnerInvalid
	}
	scope, err := admission.scope(owner.destination, ids)
	if err != nil {
		return err
	}
	pendingScope := ""
	if admission.Expected.Operation == OpTeardown {
		pendingScope = scope
	}
	actual, file, err := observeTeardownState(int(owner.directory.Fd()), owner.destination, pendingScope)
	if file != nil {
		defer file.Close()
	}
	if err != nil || actual != admission.Expected || owner.recheckCustody("") != nil {
		return errS3OwnerPending
	}
	inherited := []string{}
	if prior := owner.record.Active; prior != nil {
		if prior.Operation != OpTeardown || prior.TeardownScope != scope || prior.TeardownOrigin != admission.Origin {
			return errS3OwnerPending
		}
		inherited = append(inherited, prior.Locks...)
	}
	destination, err := s3DestinationFor(cfg)
	if err != nil || destination != owner.destination {
		return errS3OwnerInvalid
	}
	record := owner.record
	record.Version, record.Destination = 1, owner.destination
	baselineCreds := append([]s3RetainedCredential(nil), record.Credentials...)
	baselinePasses := append([][]byte(nil), record.Passwords...)
	record.Credentials = append([]s3RetainedCredential(nil), record.Credentials...)
	record.Passwords = append([][]byte(nil), record.Passwords...)
	credential := s3RetainedCredential{cfg.access, cfg.secret, cfg.token}
	found := false
	for _, retained := range record.Credentials {
		if retained == credential {
			found = true
		}
	}
	if !found {
		record.Credentials = append(record.Credentials, credential)
	}
	for _, password := range passwords {
		if len(password) == 0 {
			continue
		}
		found = false
		for _, retained := range record.Passwords {
			if bytes.Equal(retained, password) {
				found = true
			}
		}
		if !found {
			record.Passwords = append(record.Passwords, append([]byte(nil), password...))
		}
	}
	var nonce [16]byte
	if _, err := rand.Read(nonce[:]); err != nil {
		return errS3OwnerIO
	}
	record.Active = &s3OwnerIntent{ID: hex.EncodeToString(nonce[:]), Operation: OpTeardown, MayHaveWritten: owner.record.Active != nil && owner.record.Active.MayHaveWritten, Locks: inherited, TeardownScope: scope, TeardownOrigin: admission.Origin}
	owner.attemptPasses = nil
	for _, password := range passwords {
		if len(password) > 0 {
			owner.attemptPasses = append(owner.attemptPasses, append([]byte(nil), password...))
		}
	}
	record.Complete = nil
	if len(record.Credentials) > maxS3OwnerCredentials || len(record.Passwords) > maxS3OwnerPasswords {
		return errResourceLimit
	}
	_, err = owner.persist(record)
	owner.begun = true
	if err != nil {
		owner.stateFailed = true
	} else {
		owner.liveID = record.Active.ID
		owner.baselineCreds, owner.baselinePasses, owner.attemptFailed = baselineCreds, baselinePasses, false
		owner.liveCredential = credential
	}
	return err
}

// noteAttemptFailed marks the live attempt as not having succeeded, so its
// completion retains no credential or password that attempt introduced.
func (owner *s3Owner) noteAttemptFailed() {
	owner.mu.Lock()
	owner.attemptFailed = true
	owner.mu.Unlock()
}

// adoptsUncertainty reports whether an attempt's possible remote writes are
// limited to content-addressed data, index or snapshot objects and owned locks.
func adoptsUncertainty(operation int32) bool {
	switch operation {
	case OpOpen, OpSnapshot, OpListSnapshots, OpRestore, OpCheck, OpArchiveSources:
		return true
	default:
		return false
	}
}

// noteSave is write-ahead admission, not a claim that any request was sent.
func (owner *s3Owner) noteSave(lockID string) error {
	owner.mu.Lock()
	defer owner.mu.Unlock()
	if owner.closed || owner.sealing || owner.stateFailed || owner.record.Active == nil || owner.liveID == "" || owner.liveID != owner.record.Active.ID {
		return errS3OwnerInvalid
	}
	if owner.recheckCustody("") != nil {
		owner.stateFailed = true
		return errS3OwnerInvalid
	}
	record := owner.record
	active := *record.Active
	active.Locks = append([]string(nil), active.Locks...)
	changed := !active.MayHaveWritten
	active.MayHaveWritten = true
	if lockID != "" {
		if !validS3ID(lockID) {
			return errS3OwnerInvalid
		}
		found := false
		for _, id := range active.Locks {
			if id == lockID {
				found = true
			}
		}
		if !found {
			if len(active.Locks) >= maxS3OwnerLocks {
				return errResourceLimit
			}
			active.Locks = append(active.Locks, lockID)
			changed = true
		}
	}
	if active.Operation == OpAddRecoveryKey {
		if len(active.RecoveryPassword) == 0 {
			for i, p := range owner.attemptPasses {
				if i > 0 && len(p) > 0 {
					active.RecoveryPassword = append([]byte(nil), p...)
					changed = true
					break
				}
			}
		}
		if len(active.RecoveryPassword) > 0 {
			found := false
			for _, retained := range record.Passwords {
				if bytes.Equal(retained, active.RecoveryPassword) {
					found = true
					break
				}
			}
			if !found {
				record.Passwords = append(record.Passwords, append([]byte(nil), active.RecoveryPassword...))
				changed = true
			}
		}
	}
	if !changed {
		return nil
	}
	record.Active = &active
	_, err := owner.persist(record)
	if err != nil {
		owner.stateFailed = true
	}
	return err
}

func (owner *s3Owner) recordKeyID(keyID string) error {
	owner.mu.Lock()
	defer owner.mu.Unlock()
	if owner.closed || owner.sealing || owner.stateFailed || owner.record.Active == nil || owner.liveID == "" || owner.liveID != owner.record.Active.ID {
		return errS3OwnerInvalid
	}
	if owner.recheckCustody("") != nil {
		owner.stateFailed = true
		return errS3OwnerInvalid
	}
	active := *owner.record.Active
	if active.Operation != OpAddRecoveryKey {
		owner.knownKeyID = keyID
		return nil
	}
	if active.KeyID != "" && active.KeyID != keyID {
		return errS3OwnerInvalid
	}
	if active.KeyID == keyID {
		owner.knownKeyID = keyID
		return nil
	}
	active.KeyID = keyID
	record := owner.record
	record.Active = &active
	_, err := owner.persist(record)
	if err != nil {
		owner.stateFailed = true
		return err
	}
	owner.knownKeyID = keyID
	return nil
}

func (owner *s3Owner) recordRemoveKeyID(keyID string) error {
	owner.mu.Lock()
	defer owner.mu.Unlock()
	if owner.closed || owner.sealing || owner.stateFailed || owner.record.Active == nil || owner.liveID == "" || owner.liveID != owner.record.Active.ID {
		return errS3OwnerInvalid
	}
	if owner.recheckCustody("") != nil {
		owner.stateFailed = true
		return errS3OwnerInvalid
	}
	active := *owner.record.Active
	if active.Operation != OpRemoveRecoveryKey {
		return errS3OwnerInvalid
	}
	if !lowerHexID(keyID) || len(keyID) != 64 {
		return errS3OwnerInvalid
	}
	if active.KeyID != "" && active.KeyID != keyID {
		return errS3OwnerInvalid
	}
	if active.KeyID == keyID {
		owner.knownKeyID = keyID
		return nil
	}
	active.KeyID = keyID
	record := owner.record
	record.Active = &active
	_, err := owner.persist(record)
	if err != nil {
		owner.stateFailed = true
		return err
	}
	owner.knownKeyID = keyID
	return nil
}

func (owner *s3Owner) recordKeyPayload(keyID string, ciphertext []byte) error {
	owner.mu.Lock()
	defer owner.mu.Unlock()
	if owner.closed || owner.sealing || owner.stateFailed || owner.record.Active == nil || owner.liveID == "" || owner.liveID != owner.record.Active.ID {
		return errS3OwnerInvalid
	}
	if owner.recheckCustody("") != nil {
		owner.stateFailed = true
		return errS3OwnerInvalid
	}
	active := *owner.record.Active
	if active.Operation != OpAddRecoveryKey {
		owner.knownKeyID = keyID
		return nil
	}
	if !lowerHexID(keyID) || len(keyID) != 64 || len(ciphertext) == 0 {
		return errS3OwnerInvalid
	}
	if active.KeyID != "" && active.KeyID != keyID {
		return errS3OwnerInvalid
	}
	if len(active.Ciphertext) > 0 {
		if !bytes.Equal(active.Ciphertext, ciphertext) {
			return errS3OwnerInvalid
		}
		owner.knownKeyID = keyID
		return nil
	}
	active.KeyID = keyID
	active.Ciphertext = append([]byte(nil), ciphertext...)
	active.MayHaveWritten = true
	if len(active.RecoveryPassword) == 0 {
		for i, p := range owner.attemptPasses {
			if i > 0 && len(p) > 0 {
				active.RecoveryPassword = append([]byte(nil), p...)
				break
			}
		}
	}
	record := owner.record
	if len(active.RecoveryPassword) > 0 {
		found := false
		for _, retained := range record.Passwords {
			if bytes.Equal(retained, active.RecoveryPassword) {
				found = true
				break
			}
		}
		if !found {
			record.Passwords = append(record.Passwords, append([]byte(nil), active.RecoveryPassword...))
		}
	}
	record.Active = &active
	_, err := owner.persist(record)
	if err != nil {
		owner.stateFailed = true
		return err
	}
	owner.knownKeyID = keyID
	return nil
}

func (owner *s3Owner) activeAddRecoveryInfo() (string, []byte, []byte) {
	owner.mu.Lock()
	defer owner.mu.Unlock()
	if owner.record.Active != nil && owner.record.Active.Operation == OpAddRecoveryKey {
		var pw []byte
		if len(owner.record.Active.RecoveryPassword) > 0 {
			pw = append([]byte(nil), owner.record.Active.RecoveryPassword...)
		}
		var ct []byte
		if len(owner.record.Active.Ciphertext) > 0 {
			ct = append([]byte(nil), owner.record.Active.Ciphertext...)
		}
		return owner.record.Active.KeyID, pw, ct
	}
	return "", nil, nil
}

func (owner *s3Owner) recoveryHoldInfo() (string, []byte) {
	owner.mu.Lock()
	defer owner.mu.Unlock()
	if owner.record.RecoveryHold != nil {
		var pw []byte
		if len(owner.record.RecoveryHold.RecoveryPassword) > 0 {
			pw = append([]byte(nil), owner.record.RecoveryHold.RecoveryPassword...)
		}
		return owner.record.RecoveryHold.KeyID, pw
	}
	return "", nil
}

func (owner *s3Owner) saveReturned(err error) {
	if err != nil {
		owner.mu.Lock()
		owner.writeFailed = true
		owner.mu.Unlock()
	}
}

func (owner *s3Owner) hasWriteFailed() bool {
	owner.mu.Lock()
	defer owner.mu.Unlock()
	return owner.writeFailed
}

func (owner *s3Owner) ownsLock(id string) bool {
	owner.mu.Lock()
	defer owner.mu.Unlock()
	if owner.record.Active == nil {
		return false
	}
	for _, owned := range owner.record.Active.Locks {
		if owned == id {
			return true
		}
	}
	return false
}

func (owner *s3Owner) lockRemoved(id string, err error) {
	if err == nil {
		owner.mu.Lock()
		owner.lockAcks[id] = true
		owner.mu.Unlock()
	}
}

// Call only after actual consequential operation/backend work and cleanup drain.
func (owner *s3Owner) complete() (published bool, err error) {
	owner.mu.Lock()
	defer owner.mu.Unlock()
	if owner.record.Active == nil || owner.closed || !owner.primaryDrained || owner.cleanupActive || owner.liveID == "" || owner.liveID != owner.record.Active.ID {
		return false, errS3OwnerInvalid
	}
	if owner.preservedAddIntent != nil {
		if owner.writeFailed || owner.stateFailed {
			return false, errS3OwnerPending
		}
		var remainingLocks []string
		hasUnacked := false
		for _, id := range owner.record.Active.Locks {
			if !owner.lockAcks[id] {
				hasUnacked = true
				remainingLocks = append(remainingLocks, id)
			}
		}
		if hasUnacked {
			return false, errS3OwnerPending
		}
		record := owner.record
		addIntent := *owner.preservedAddIntent
		addIntent.Locks = remainingLocks
		record.Active = &addIntent
		record.Complete = nil
		published, err := owner.persist(record)
		if err == nil {
			owner.preservedAddIntent = nil
		}
		return published, err
	}
	if owner.preservedRemoveIntent != nil {
		if owner.writeFailed || owner.stateFailed {
			return false, errS3OwnerPending
		}
		var remainingLocks []string
		hasUnacked := false
		for _, id := range owner.record.Active.Locks {
			if !owner.lockAcks[id] {
				hasUnacked = true
				remainingLocks = append(remainingLocks, id)
			}
		}
		if hasUnacked {
			return false, errS3OwnerPending
		}
		record := owner.record
		removeIntent := *owner.preservedRemoveIntent
		removeIntent.Locks = remainingLocks
		record.Active = &removeIntent
		record.Complete = nil
		published, err := owner.persist(record)
		if err == nil {
			owner.preservedRemoveIntent = nil
		}
		return published, err
	}
	if owner.writeFailed || owner.stateFailed {
		return false, errS3OwnerPending
	}
	if owner.attemptFailed && (owner.record.Active.Operation == OpTeardown || owner.record.Active.Operation == OpRetention) && owner.record.Active.MayHaveWritten {
		return false, errS3OwnerPending
	}
	for _, id := range owner.record.Active.Locks {
		if !owner.lockAcks[id] {
			return false, errS3OwnerPending
		}
	}
	if owner.attemptFailed && owner.record.Active != nil && (owner.record.Active.Operation == OpAddRecoveryKey || owner.record.Active.Operation == OpRemoveRecoveryKey) && owner.record.Active.MayHaveWritten {
		return false, errS3OwnerPending
	}
	record := owner.record
	if !owner.attemptFailed && owner.record.Active != nil && owner.record.Active.Operation == OpAddRecoveryKey {
		keyID := owner.record.Active.KeyID
		if keyID == "" {
			keyID = owner.knownKeyID
		}
		var recPw []byte
		if len(owner.record.Active.RecoveryPassword) > 0 {
			recPw = owner.record.Active.RecoveryPassword
		} else if len(owner.attemptPasses) > 1 {
			recPw = owner.attemptPasses[1]
		}
		ciphertext := owner.record.Active.Ciphertext
		if keyID == "" || !lowerHexID(keyID) || len(keyID) != 64 || len(recPw) == 0 || len(ciphertext) == 0 {
			return false, errS3OwnerPending
		}
		if owner.record.Active.RotationCustody {
			record.RecoveryHold = &s3RecoveryHold{
				KeyID:            keyID,
				RecoveryPassword: append([]byte(nil), recPw...),
				Ciphertext:       append([]byte(nil), ciphertext...),
			}
		} else {
			record.RecoveryHold = nil
		}
	} else if !owner.attemptFailed && owner.record.Active != nil && owner.record.Active.Operation == OpRemoveRecoveryKey {
		record.RecoveryHold = nil
	}
	record.Complete = &s3OwnerCompletion{ID: record.Active.ID, Operation: record.Active.Operation, TeardownScope: record.Active.TeardownScope, TeardownOrigin: record.Active.TeardownOrigin, TeardownReconciled: record.Active.Operation == OpTeardown && owner.teardownReconciled && !owner.attemptFailed && owner.primaryDrained}
	record.Active = nil
	// A failed attempt that wrote nothing (a mistyped recovery key, a refused
	// credential) adds no retained material; one that may have written keeps
	// it in custody. A successful attempt's credential is the only one still
	// needed, so rotating or session credentials cannot exhaust the destination.
	if owner.attemptFailed && !owner.record.Active.MayHaveWritten && len(owner.baselineCreds) != 0 && len(owner.baselinePasses) != 0 {
		record.Credentials = owner.baselineCreds
		record.Passwords = owner.baselinePasses
	} else if !owner.attemptFailed {
		record.Credentials = []s3RetainedCredential{owner.liveCredential}
		if record.Complete != nil {
			switch record.Complete.Operation {
			case OpAddRecoveryKey:
				var candidate []byte
				if record.RecoveryHold != nil && len(record.RecoveryHold.RecoveryPassword) > 0 {
					candidate = record.RecoveryHold.RecoveryPassword
				} else if len(owner.record.Active.RecoveryPassword) > 0 {
					candidate = owner.record.Active.RecoveryPassword
				} else if len(owner.attemptPasses) > 1 {
					candidate = owner.attemptPasses[1]
				}
				var newPasses [][]byte
				if len(owner.attemptPasses) > 0 && len(owner.attemptPasses[0]) > 0 {
					newPasses = append(newPasses, append([]byte(nil), owner.attemptPasses[0]...))
				}
				if len(candidate) > 0 {
					newPasses = append(newPasses, append([]byte(nil), candidate...))
				}
				record.Passwords = newPasses
			case OpRemoveRecoveryKey:
				record.RecoveryHold = nil
				for _, p := range owner.attemptPasses {
					if len(p) == 0 {
						continue
					}
					found := false
					for _, retained := range record.Passwords {
						if bytes.Equal(retained, p) {
							found = true
							break
						}
					}
					if !found {
						record.Passwords = append(record.Passwords, append([]byte(nil), p...))
					}
				}
			}
		}
	}
	previous := owner.record.Passwords
	published, err = owner.persist(record)
	if err == nil {
		for _, retired := range previous {
			kept := false
			for _, password := range record.Passwords {
				kept = kept || (len(password) != 0 && len(retired) != 0 && &password[0] == &retired[0])
			}
			if !kept {
				zeroBytes(retired)
			}
		}
	}
	return published, err
}

func (owner *s3Owner) Close() error {
	if owner.owned != nil {
		return owner.closeOwned()
	}
	owner.mu.Lock()
	if owner.closed {
		owner.mu.Unlock()
		return nil
	}
	if owner.begun && !owner.primaryDrained || owner.cleanupActive {
		owner.mu.Unlock()
		return errS3OwnerBusy
	}
	owner.closed = true
	var closeErr error
	if owner.stateFile != nil {
		closeErr = errors.Join(closeErr, owner.stateFile.Close())
		owner.stateFile = nil
	}
	if owner.root != nil {
		closeErr = errors.Join(closeErr, owner.root.Close())
		owner.root = nil
	}
	if owner.directory != nil {
		closeErr = errors.Join(closeErr, closeRootedPrivateDirectory(owner.directory))
		owner.directory = nil
	}
	for _, password := range owner.record.Passwords {
		zeroBytes(password)
	}
	for _, password := range owner.attemptPasses {
		zeroBytes(password)
	}
	if owner.record.Active != nil {
		if len(owner.record.Active.RecoveryPassword) > 0 {
			zeroBytes(owner.record.Active.RecoveryPassword)
		}
		if len(owner.record.Active.Ciphertext) > 0 {
			zeroBytes(owner.record.Active.Ciphertext)
		}
	}
	if owner.preservedAddIntent != nil {
		if len(owner.preservedAddIntent.RecoveryPassword) > 0 {
			zeroBytes(owner.preservedAddIntent.RecoveryPassword)
		}
		if len(owner.preservedAddIntent.Ciphertext) > 0 {
			zeroBytes(owner.preservedAddIntent.Ciphertext)
		}
	}
	if owner.preservedRemoveIntent != nil {
		if len(owner.preservedRemoveIntent.RecoveryPassword) > 0 {
			zeroBytes(owner.preservedRemoveIntent.RecoveryPassword)
		}
		if len(owner.preservedRemoveIntent.Ciphertext) > 0 {
			zeroBytes(owner.preservedRemoveIntent.Ciphertext)
		}
	}
	if owner.record.RecoveryHold != nil {
		if len(owner.record.RecoveryHold.RecoveryPassword) > 0 {
			zeroBytes(owner.record.RecoveryHold.RecoveryPassword)
		}
		if len(owner.record.RecoveryHold.Ciphertext) > 0 {
			zeroBytes(owner.record.RecoveryHold.Ciphertext)
		}
	}
	owner.attemptPasses = nil
	owner.preservedAddIntent = nil
	owner.preservedRemoveIntent = nil
	owner.mu.Unlock()
	s3OwnersMu.Lock()
	if s3Owners[owner.destination] == owner {
		delete(s3Owners, owner.destination)
	}
	s3OwnersMu.Unlock()
	return closeErr
}

func (owner *s3Owner) isUnsealed() bool {
	owner.mu.Lock()
	defer owner.mu.Unlock()
	return !owner.closed && !owner.sealing
}
