// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package main

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/xml"
	"errors"
	"io"
	"net/http"
	"net/url"
	"strconv"
	"strings"

	"github.com/minio/minio-go/v7/pkg/signer"
	"github.com/restic/restic/internal/backend"
)

func s3ExternalError(ctx context.Context, err error) error {
	if ctx.Err() != nil {
		return ctx.Err()
	}
	if errors.Is(err, context.Canceled) {
		return context.Canceled
	}
	if errors.Is(err, context.DeadlineExceeded) {
		return context.DeadlineExceeded
	}
	if errors.Is(err, errHTTPLimit) {
		return errResourceLimit
	}
	if errors.Is(err, errHTTPUnsupported) {
		return errS3Unavailable
	}
	return errS3IO
}

func s3ConsumerError(ctx context.Context, err error) error {
	if errors.Is(err, errResourceLimit) {
		return errResourceLimit
	}
	if ctx.Err() != nil {
		return ctx.Err()
	}
	if errors.Is(err, context.Canceled) {
		return context.Canceled
	}
	if errors.Is(err, context.DeadlineExceeded) {
		return context.DeadlineExceeded
	}
	if errors.Is(err, errS3InitExisting) {
		return errS3InitExisting
	}
	return errS3Callback
}

// Decode exactly one bounded XML document. DTDs and trailing documents are not
// part of S3's response contract. xml.Decoder never resolves external entities.
func decodeS3XML(encoded []byte, target any) error {
	// DecodeElement would otherwise ignore an inner Directive token. Inspect
	// the bounded wire document first, including inside descriptive elements.
	scan := xml.NewDecoder(bytes.NewReader(encoded))
	for {
		token, err := scan.Token()
		if err == io.EOF {
			break
		}
		if err != nil {
			return errS3Protocol
		}
		if _, directive := token.(xml.Directive); directive {
			return errS3Protocol
		}
	}
	decoder := xml.NewDecoder(bytes.NewReader(encoded))
	for {
		token, err := decoder.Token()
		if err != nil {
			return errS3Protocol
		}
		switch typed := token.(type) {
		case xml.ProcInst:
			if typed.Target != "xml" {
				return errS3Protocol
			}
		case xml.CharData:
			if len(bytes.TrimSpace(typed)) != 0 {
				return errS3Protocol
			}
		case xml.StartElement:
			if err := decoder.DecodeElement(target, &typed); err != nil {
				return errS3Protocol
			}
			for {
				tail, err := decoder.Token()
				if err == io.EOF {
					return nil
				}
				if err != nil {
					return errS3Protocol
				}
				if text, ok := tail.(xml.CharData); !ok || len(bytes.TrimSpace(text)) != 0 {
					return errS3Protocol
				}
			}
		default:
			return errS3Protocol
		}
	}
}

func s3ErrorResponse(method string, status int, body []byte) error {
	if status == http.StatusUnauthorized || status == http.StatusForbidden {
		return errS3Denied
	}
	// Only a create-only write is conditional, so this is an existing object.
	if status == http.StatusPreconditionFailed && method == http.MethodPut {
		return errS3ObjectExists
	}
	if status == http.StatusRequestedRangeNotSatisfiable {
		return errS3Range
	}
	if status >= 500 || status == http.StatusTooManyRequests {
		return errS3IO
	}
	if status == http.StatusNotFound && method == http.MethodHead && len(body) == 0 {
		return errS3Ambiguous404
	}
	var response struct {
		XMLName xml.Name `xml:"Error"`
		Code    []string `xml:"Code"`
	}
	if decodeS3XML(body, &response) != nil || len(response.Code) != 1 {
		return errS3Protocol
	}
	if status == http.StatusNotFound {
		switch response.Code[0] {
		case "NoSuchKey":
			return errS3NotFound
		case "NoSuchBucket":
			return errS3BucketMissing
		}
	}
	return errS3Protocol
}

// Only the pinned signer runs here. No SDK client may discover a region,
// credential, session or alternate authority. Each call owns a complete reply.
func (be *s3Backend) request(ctx context.Context, method, key string, query url.Values, payload []byte, rangeValue string) (*http.Response, []byte, error) {
	return be.send(ctx, method, key, query, payload, rangeValue, false)
}

func (be *s3Backend) send(ctx context.Context, method, key string, query url.Values, payload []byte, rangeValue string, createOnly bool) (*http.Response, []byte, error) {
	if ctx.Err() != nil {
		return nil, nil, ctx.Err()
	}
	endpoint, _ := url.Parse(be.cfg.endpoint) // configuration was validated before admission
	// The signer signs the URL's host, while Foundation sends Host without a
	// default port. Providers that verify the received Host (R2) refuse the
	// mismatch, so a default port is never spelled in the signed host.
	if endpoint.Port() == "443" {
		endpoint.Host = strings.TrimSuffix(endpoint.Host, ":443")
	}
	endpoint.Path = "/" + be.cfg.bucket
	if key != "" {
		endpoint.Path += "/" + key
	}
	endpoint.RawQuery = query.Encode()
	var reader io.Reader
	if method == http.MethodPut {
		reader = bytes.NewReader(payload)
	}
	req, err := http.NewRequestWithContext(ctx, method, endpoint.String(), reader)
	if err != nil {
		return nil, nil, errS3Config
	}
	if rangeValue != "" {
		req.Header.Set("Range", rangeValue)
	}
	if method == http.MethodPut {
		req.Header.Set("Content-Type", "application/octet-stream")
	}
	if createOnly {
		req.Header.Set("If-None-Match", "*")
	}
	digest := sha256.Sum256(payload)
	req.Header.Set("X-Amz-Content-Sha256", hex.EncodeToString(digest[:]))
	req = signer.SignV4(*req, be.cfg.access, be.cfg.secret, be.cfg.token, be.cfg.region)
	reply, err := be.transport.RoundTrip(req)
	if err != nil {
		return nil, nil, s3ExternalError(ctx, err)
	}
	if reply == nil || reply.Body == nil {
		return nil, nil, errS3Protocol
	}
	limit := maxHTTPObjectBytes
	if query.Get("list-type") == "2" {
		limit = maxHTTPListBytes
	}
	if reply.StatusCode < 200 || reply.StatusCode >= 300 {
		limit = maxHTTPErrorBytes
	}
	body, readErr := io.ReadAll(io.LimitReader(reply.Body, int64(limit)+1))
	closeErr := reply.Body.Close()
	if len(body) > limit {
		zeroBytes(body)
		return nil, nil, errResourceLimit
	}
	if readErr != nil || closeErr != nil {
		zeroBytes(body)
		return nil, nil, s3ExternalError(ctx, errS3IO)
	}
	refuse := func(err error) (*http.Response, []byte, error) { zeroBytes(body); return nil, nil, err }
	seen, headerBytes := make(map[string]bool), 0
	for name, values := range reply.Header {
		lower := strings.ToLower(name)
		if seen[lower] || !validHTTPHeader(name, values) {
			return refuse(errS3Protocol)
		}
		seen[lower] = true
		headerBytes += len(name)
		for _, value := range values {
			headerBytes += len(value)
		}
		if headerBytes > maxHTTPHeaderBytes {
			return refuse(errResourceLimit)
		}
	}
	if encoding := reply.Header.Get("Content-Encoding"); encoding != "" && !strings.EqualFold(encoding, "identity") {
		return refuse(errS3Protocol)
	}
	if strings.HasPrefix(strings.ToLower(reply.Header.Get("Content-Type")), "multipart/") {
		return refuse(errS3Protocol)
	}
	if length, present := reply.Header["Content-Length"]; present {
		if len(length) != 1 {
			return refuse(errS3Protocol)
		}
		count, err := strconv.ParseInt(length[0], 10, 64)
		if err != nil || count < 0 || method != http.MethodHead && count != int64(len(body)) {
			return refuse(errS3Protocol)
		}
	}
	if reply.StatusCode < 200 || reply.StatusCode >= 300 {
		return refuse(s3ErrorResponse(method, reply.StatusCode, body))
	}
	return reply, body, nil
}

func (be *s3Backend) Save(ctx context.Context, handle backend.Handle, reader backend.RewindReader) error {
	owned, done, err := be.enter(ctx)
	if err != nil {
		return err
	}
	defer done()
	key, err := be.filename(handle)
	if err != nil {
		return err
	}
	if reader == nil {
		return errS3Handle
	}
	length := reader.Length()
	if length < 0 || length > maxHTTPObjectBytes {
		return errResourceLimit
	}
	if owned.Err() != nil {
		return owned.Err()
	}
	body, err := io.ReadAll(io.LimitReader(reader, length+1))
	defer zeroBytes(body)
	if err != nil {
		return s3ExternalError(owned, err)
	}
	if int64(len(body)) != length {
		return errS3Protocol
	}
	// An append-only destination never replaces an object; only locks are mutable.
	createOnly := be.cfg.appendOnly && handle.Type != backend.LockFile
	reply, response, err := be.send(owned, http.MethodPut, key, nil, body, "", createOnly)
	defer zeroBytes(response)
	if err != nil {
		return err
	}
	if reply.StatusCode != http.StatusOK || len(response) != 0 {
		return errS3Protocol
	}
	return nil
}

func validS3Range(header string, offset, length, delivered int64) bool {
	if !strings.HasPrefix(header, "bytes ") {
		return false
	}
	interval, total, ok := strings.Cut(strings.TrimPrefix(header, "bytes "), "/")
	if !ok {
		return false
	}
	start, end, ok := strings.Cut(interval, "-")
	if !ok {
		return false
	}
	a, e1 := strconv.ParseInt(start, 10, 64)
	b, e2 := strconv.ParseInt(end, 10, 64)
	n, e3 := strconv.ParseInt(total, 10, 64)
	if e1 != nil || e2 != nil || e3 != nil || a != offset || b < a || n <= b || n > maxHTTPObjectBytes || b-a+1 != delivered {
		return false
	}
	if length > 0 {
		return delivered == length
	}
	return b == n-1
}

func (be *s3Backend) load(ctx context.Context, key string, length int, offset int64) ([]byte, error) {
	if length < 0 || offset < 0 || offset > maxHTTPObjectBytes || int64(length) > maxHTTPObjectBytes-offset {
		return nil, errS3Range
	}
	rangeValue := ""
	if length > 0 {
		rangeValue = "bytes=" + strconv.FormatInt(offset, 10) + "-" + strconv.FormatInt(offset+int64(length)-1, 10)
	} else if offset > 0 {
		rangeValue = "bytes=" + strconv.FormatInt(offset, 10) + "-"
	}
	reply, body, err := be.request(ctx, http.MethodGet, key, nil, nil, rangeValue)
	if err != nil {
		return nil, err
	}
	if rangeValue != "" {
		if reply.StatusCode != http.StatusPartialContent || !validS3Range(reply.Header.Get("Content-Range"), offset, int64(length), int64(len(body))) {
			zeroBytes(body)
			return nil, errS3Range
		}
	} else if reply.StatusCode != http.StatusOK || reply.Header.Get("Content-Range") != "" {
		zeroBytes(body)
		return nil, errS3Protocol
	}
	return body, nil
}

func (be *s3Backend) Load(ctx context.Context, handle backend.Handle, length int, offset int64, consume func(io.Reader) error) error {
	owned, done, err := be.enter(ctx)
	if err != nil {
		return err
	}
	defer done()
	key, err := be.filename(handle)
	if err != nil {
		return err
	}
	if consume == nil {
		return errS3Handle
	}
	body, err := be.load(owned, key, length, offset)
	if err != nil {
		return err
	}
	defer zeroBytes(body)
	if err := consume(bytes.NewReader(body)); err != nil {
		return s3ConsumerError(owned, err)
	}
	return nil
}

func (be *s3Backend) Stat(ctx context.Context, handle backend.Handle) (backend.FileInfo, error) {
	owned, done, err := be.enter(ctx)
	if err != nil {
		return backend.FileInfo{}, err
	}
	defer done()
	key, err := be.filename(handle)
	if err != nil {
		return backend.FileInfo{}, err
	}
	reply, body, err := be.request(owned, http.MethodHead, key, nil, nil, "")
	defer zeroBytes(body)
	if errors.Is(err, errS3Ambiguous404) {
		data, err := be.load(owned, key, 0, 0)
		defer zeroBytes(data)
		if err != nil {
			return backend.FileInfo{}, err
		}
		return backend.FileInfo{Name: handle.Name, Size: int64(len(data))}, nil
	}
	if err != nil {
		return backend.FileInfo{}, err
	}
	length, parseErr := strconv.ParseInt(reply.Header.Get("Content-Length"), 10, 64)
	if reply.StatusCode != http.StatusOK || len(body) != 0 || parseErr != nil || length < 0 || length > maxHTTPObjectBytes {
		return backend.FileInfo{}, errS3Protocol
	}
	return backend.FileInfo{Name: handle.Name, Size: length}, nil
}

type teardownScopeKey struct{}

func withTeardownScope(ctx context.Context) context.Context {
	return context.WithValue(ctx, teardownScopeKey{}, true)
}

func isTeardownScope(ctx context.Context) bool {
	v, ok := ctx.Value(teardownScopeKey{}).(bool)
	return ok && v
}

type retentionScopeKey struct{}

func withRetentionScope(ctx context.Context) context.Context {
	return context.WithValue(ctx, retentionScopeKey{}, true)
}

func isRetentionScope(ctx context.Context) bool {
	v, ok := ctx.Value(retentionScopeKey{}).(bool)
	return ok && v
}

// This primitive is not an owned-lock capability. The future operation layer
// must gate it with durable current-attempt IDs. Non-lock deletion enters only
// under teardown scope or retention scope, and only for snapshot, pack, and
// index. Config and key never enter.
func (be *s3Backend) Remove(ctx context.Context, handle backend.Handle) error {
	owned, done, err := be.enter(ctx)
	if err != nil {
		return err
	}
	defer done()
	key, err := be.filename(handle)
	if err != nil {
		return err
	}
	if handle.Type != backend.LockFile {
		if (!isTeardownScope(ctx) && !isRetentionScope(ctx)) || (handle.Type != backend.SnapshotFile && handle.Type != backend.PackFile && handle.Type != backend.IndexFile) {
			return errS3Denied
		}
	}
	reply, body, err := be.request(owned, http.MethodDelete, key, nil, nil, "")
	defer zeroBytes(body)
	if errors.Is(err, errS3NotFound) {
		return nil
	}
	if err != nil {
		return err
	}
	if reply.StatusCode != http.StatusNoContent || len(body) != 0 {
		return errS3Protocol
	}
	return nil
}

func (be *s3Backend) removeExactKeyObject(ctx context.Context, keyID string, maintenance bool) error {
	if !validS3ID(keyID) {
		return errS3Handle
	}
	if be.cfg.appendOnly && !maintenance {
		return errS3Denied
	}
	owned, done, err := be.enter(ctx)
	if err != nil {
		return err
	}
	defer done()
	handle := backend.Handle{Type: backend.KeyFile, Name: keyID}
	key, err := be.filename(handle)
	if err != nil {
		return err
	}
	reply, body, err := be.request(owned, http.MethodDelete, key, nil, nil, "")
	defer zeroBytes(body)
	if err != nil {
		return err
	}
	if reply.StatusCode != http.StatusNoContent || len(body) != 0 {
		return errS3Protocol
	}
	return nil
}
