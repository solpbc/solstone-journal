#!/bin/bash
# usage: golist_check.sh <gover> <moddir> <pkg> <tags> <out_prefix> <target:GOOS/GOARCH/CGO>...
gover=$1 dir=$2 pkg=$3 tags=$4 pfx=$5; shift 5
. ${GOLIC_ROOT:-/var/tmp/golic-r1}/scripts/env.sh $gover
cd "$dir" || exit 1
for spec in "$@"; do
  IFS=/ read -r name goos goarch cgo <<<"$spec"
  GOOS=$goos GOARCH=$goarch CGO_ENABLED=$cgo GOFLAGS="-mod=readonly -buildvcs=false" go list -deps -tags "$tags" -f '{{with .Module}}{{.Path}}	{{.Version}}	{{$.ImportPath}}	{{$.Dir}}{{end}}' "$pkg" > "$pfx.$name.pk" 2> "$pfx.$name.err"
  echo "$name rc=$?"
done
