# Third-party license sets for restic 0.19.0 and rclone 1.74.4

This directory holds the license texts for everything linked into the upstream
restic and rclone release binaries that ship inside the journal packages. These
are the unmodified upstream binaries, not rebuilds.

- `restic/` and `rclone/` each contain:
  - `LICENSE`: the tool's own license (for rclone, a copy of its `COPYING`).
  - `self/` (rclone only): third-party licenses found inside rclone's own
    source tree. restic's one embedded snippet (MIT code from go-winio, inline
    in a Windows-only source file) is covered by the go-winio LICENSE under
    `deps/`, as `attribution.json` notes.
  - `deps/<module path, "/" replaced by "__">@<version>/`: every license,
    NOTICE, PATENTS and AUTHORS file that applies to the linked packages of that
    module, copied byte for byte from the module source.
  - `go/LICENSE` and `go/PATENTS`, from the Go toolchain that built the
    binaries. The Go standard library is linked into every binary.
  - `supplied/`: full license texts that a dependency references but does not
    include (see "Supplied texts" below).
  - `attribution.json`: one entry per module, sorted by module path, with the
    version, h1 sum, targets, file list, detected licenses and an SPDX
    expression where one could be confirmed.
- `reproduce/`: the scripts used here, plus the raw `go version -m` output for
  each binary under `reproduce/buildinfo/`.

## Inputs

Release assets came from the GitHub releases (`gh release download`). Both
checksum files have valid signatures from the signing keys listed in each
project's tagged docs:

- restic `SHA256SUMS.asc`: key `CF8F18F2844575973F79D4E191A6868BD3F7A907`
  (listed in restic v0.19.0 `doc/090_participating.rst`)
- rclone `SHA256SUMS` (clearsigned): key `FBF737ECE9F8AB18604BD2AC93935E02FF3B54FA`
  (listed in rclone v1.74.4 `docs/content/release_signing.md`)

| Asset | SHA-256 |
|---|---|
| restic_0.19.0_linux_amd64.bz2 | `13176fe6d89d4357947a2cd107218ab2873a5f9d8e1ac2d4cd1c8e07e6839c21` |
| restic_0.19.0_linux_arm64.bz2 | `e522ce6bf748d753fee8093e8ec59359972cf5b6bc65fc7c7cf38ae952351d91` |
| restic_0.19.0_darwin_arm64.bz2 | `1475397bf759ef4be16a77b19dec650bdbfec00d2cacd82005553411cdd37997` |
| restic_0.19.0_windows_amd64.zip | `6fa4219a70b1b5d1c429bb106a7f97f3d2a5aab74494db2e490b625edc486d8f` |
| rclone-v1.74.4-linux-amd64.zip | `fe435e0c36228e7c2f116a8701f01127bb1f694005fc11d1f27186c8bca4115d` |
| rclone-v1.74.4-linux-arm64.zip | `97685285c9ad6a0cf17d5844115d2a67245af6444db672187074bd9c358de419` |
| rclone-v1.74.4-osx-arm64.zip | `c2100e2d4a4b3be04c55cd45380cafe7647e1ad772bb055f52f00876ed701167` |
| rclone-v1.74.4-windows-amd64.zip | `ef097ef9de37a57feb7d9f9c7afb34148ad3c65be8025f1d8f7f521554a701ea` |

| Extracted binary | SHA-256 | Built with |
|---|---|---|
| restic (linux/amd64) | `ae7fe58ab3511f830fd31d157158620b209522ff1332b119199d2e938d72338c` | go1.26.4 |
| restic (linux/arm64) | `e5277c64460889e289c061a41191427127daadaed200910431b4284cf8c87172` | go1.26.4 |
| restic (darwin/arm64) | `f6c965a0f7f59464614130d79246479d48e2aa6780c34d27df6e48c8ee0308bd` | go1.26.4 |
| restic_0.19.0_windows_amd64.exe | `40576f77c1d40245a9f4af92a0b37b0d2514e6be0dffbf16ca8855820c13693e` | go1.26.4 |
| rclone (linux/amd64) | `9f56ca5edfac24a3ed37226c2ba1de69f1ec9e05fa2526cddee5cd97e202be6b` | go1.26.5 |
| rclone (linux/arm64) | `e062d30596c386046c8471f3035611d0438c22ef5fa42d3d6128dbf48ed5c76c` | go1.26.5 |
| rclone (osx/arm64) | `79dde6096c8d92c31495faac36fc764e3b3d557ee8569ce16c9fb07ce808024e` | go1.26.5 |
| rclone.exe (windows/amd64) | `492648a3867dbc620188a305e05ff3216aecbf4622bf1a6b5b978ed9c939e18c` | go1.26.5 |

Build settings from `go version -m`:

- restic, all targets: `CGO_ENABLED=0`, `-tags=selfupdate,disable_grpc_modules`,
  `-ldflags="-s -w"`. The main module version is `(devel)` with no VCS stamp.
- rclone, all targets: `-tags=cmount`, `-trimpath=true`,
  `vcs.revision=5bc93a2a7ab0ebd0a11352bc4968eabeffb18027`. `CGO_ENABLED=1` on
  osx/arm64 and `0` elsewhere. The linux builds report `vcs.modified=true`
  (`v1.74.4+dirty`); osx and windows report `false`.
- The macOS binaries link only OS libraries (`libSystem`, `libresolv`,
  `CoreFoundation`, `Security`), per the imported-library list from
  `reproduce/macholibs/`. The rclone macOS build can also load a FUSE library
  (macFUSE or FUSE-T) at run time for `rclone mount`; that library is not
  bundled.

## Tools

- Go go1.26.4 and go1.26.5, linux-amd64 tarballs from go.dev. SHA-256
  `1153d3d50e0ac764b447adfe05c2bcf08e889d42a02e0fe0259bd47f6733ad7f` and
  `5c2c3b16caefa1d968a94c1daca04a7ca301a496d9b086e17ad77bb81393f053`, checked
  against `https://go.dev/dl/?mode=json&include=all`. Each toolchain matches its
  binaries' build info exactly.
- Module downloads used `GOPROXY=https://proxy.golang.org,direct`,
  `GOSUMDB=sum.golang.org` and `GOTOOLCHAIN=local`.
- `github.com/google/licenseclassifier/v2 v2.0.0`
  (`h1:1Y57HHILNf4m0ABuMVb6xk4vAJYEUO0gDxNpog0pyeA=`), through the small
  wrapper in `reproduce/classify/`. The SPDX fields come from this classifier,
  threshold 0.9, plus the reviewed overrides listed in `reproduce/assemble.py`.
- Python 3.12.3, GnuPG 2.4.4, gh 2.97.0, Linux x86_64.

## Commands

```sh
export GOLIC_ROOT=/var/tmp/golic-r1      # any empty scratch directory
R=$GOLIC_ROOT
mkdir -p $R/scripts $R/dl/restic $R/dl/rclone $R/bins/restic $R/bins/rclone $R/tc $R/bi $R/work/empty $R/bin $R/dl/supplied
cp -r reproduce/* $R/scripts/

# 1. release assets, checksums, signatures
(cd $R/dl/restic && gh release download v0.19.0 -R restic/restic \
   -p 'restic_0.19.0_linux_amd64.bz2' -p 'restic_0.19.0_linux_arm64.bz2' \
   -p 'restic_0.19.0_darwin_arm64.bz2' -p 'restic_0.19.0_windows_amd64.zip' \
   -p SHA256SUMS -p SHA256SUMS.asc && sha256sum -c --ignore-missing SHA256SUMS)
(cd $R/dl/rclone && gh release download v1.74.4 -R rclone/rclone \
   -p 'rclone-v1.74.4-linux-amd64.zip' -p 'rclone-v1.74.4-linux-arm64.zip' \
   -p 'rclone-v1.74.4-osx-arm64.zip' -p 'rclone-v1.74.4-windows-amd64.zip' \
   -p SHA256SUMS && sha256sum -c --ignore-missing SHA256SUMS)
export GNUPGHOME=$R/gnupg; mkdir -m 700 -p $GNUPGHOME
gpg --keyserver hkps://keys.openpgp.org --recv-keys FBF737ECE9F8AB18604BD2AC93935E02FF3B54FA
gpg --keyserver hkps://keyserver.ubuntu.com --recv-keys CF8F18F2844575973F79D4E191A6868BD3F7A907
gpg --verify $R/dl/restic/SHA256SUMS.asc $R/dl/restic/SHA256SUMS
gpg --verify $R/dl/rclone/SHA256SUMS

# 2. extract binaries (each archive into its own directory)
for t in linux_amd64 linux_arm64 darwin_arm64; do
  bzip2 -dc $R/dl/restic/restic_0.19.0_$t.bz2 > $R/bins/restic/restic_$t; done
mkdir -p $R/x/restic_win && unzip -q $R/dl/restic/restic_0.19.0_windows_amd64.zip -d $R/x/restic_win
cp $R/x/restic_win/restic_0.19.0_windows_amd64.exe $R/bins/restic/restic_windows_amd64.exe
for t in linux-amd64 linux-arm64 osx-arm64 windows-amd64; do
  mkdir -p $R/x/rclone_$t && unzip -q $R/dl/rclone/rclone-v1.74.4-$t.zip -d $R/x/rclone_$t; done
for t in linux-amd64 linux-arm64 osx-arm64; do
  cp $R/x/rclone_$t/rclone-v1.74.4-$t/rclone $R/bins/rclone/rclone_$t; done
cp $R/x/rclone_windows-amd64/rclone-v1.74.4-windows-amd64/rclone.exe $R/bins/rclone/rclone_windows-amd64.exe
sha256sum $R/bins/*/*                      # compare with the table above

# 3. pinned toolchains (hashes from the Tools section, which match go.dev/dl JSON)
printf '%s  %s\n' \
  1153d3d50e0ac764b447adfe05c2bcf08e889d42a02e0fe0259bd47f6733ad7f go1.26.4.linux-amd64.tar.gz \
  5c2c3b16caefa1d968a94c1daca04a7ca301a496d9b086e17ad77bb81393f053 go1.26.5.linux-amd64.tar.gz > $R/tc/sums
for v in 1.26.4 1.26.5; do (cd $R/tc && curl -fsSLO https://go.dev/dl/go$v.linux-amd64.tar.gz); done
(cd $R/tc && sha256sum -c sums) && for v in 1.26.4 1.26.5; do
  mkdir $R/tc/go$v && tar -C $R/tc/go$v -xzf $R/tc/go$v.linux-amd64.tar.gz; done

# 4. build info -> JSON, per-target diff
(. $R/scripts/env.sh 1.26.5; for f in $R/bins/*/*; do go version -m "$f" > $R/bi/$(basename $f).txt; done)
python3 -I $R/scripts/parse_bi.py $R/bi restic > $R/bi/restic.json
python3 -I $R/scripts/parse_bi.py $R/bi rclone > $R/bi/rclone.json
python3 -I $R/scripts/diff_targets.py $R/bi/restic.json
python3 -I $R/scripts/diff_targets.py $R/bi/rclone.json
(cd $R/scripts/macholibs && . $R/scripts/env.sh 1.26.5 && for f in $R/bins/restic/restic_darwin_arm64 $R/bins/rclone/rclone_osx-arm64; do
  go run -buildvcs=false . $f; done)

# 5. download exactly the build-info module versions (+ the tagged main module), verify h1
python3 -I -c 'import json,sys
for tool,main in (("restic","github.com/restic/restic@v0.19.0"),("rclone","github.com/rclone/rclone@v1.74.4")):
    d=json.load(open(f"'$R'/bi/{tool}.json")); u={m:v[0] for t in d.values() for m,v in t["deps"].items()}
    open(f"'$R'/bi/{tool}.mods","w").write(main+"\n"+"".join(f"{m}@{u[m]}\n" for m in sorted(u)))'
(cd $R/work/empty && . $R/scripts/env.sh 1.26.4 && go mod download -json $(cat $R/bi/restic.mods) > $R/work/restic.dl.json)
(cd $R/work/empty && . $R/scripts/env.sh 1.26.5 && go mod download -json $(cat $R/bi/rclone.mods) > $R/work/rclone.dl.json)
python3 -I $R/scripts/verify_dl.py $R/bi/restic.json $R/work/restic.dl.json
python3 -I $R/scripts/verify_dl.py $R/bi/rclone.json $R/work/rclone.dl.json

# 6. linked packages per target from the tagged sources; module sets must equal build info
$R/scripts/golist_check.sh 1.26.4 $R/modcache/github.com/restic/restic@v0.19.0 ./cmd/restic \
  selfupdate,disable_grpc_modules $R/work/restic.list \
  linux_amd64/linux/amd64/0 linux_arm64/linux/arm64/0 darwin_arm64/darwin/arm64/0 windows_amd64/windows/amd64/0
$R/scripts/golist_check.sh 1.26.5 $R/modcache/github.com/rclone/rclone@v1.74.4 . cmount $R/work/rclone.list \
  linux_amd64/linux/amd64/0 linux_arm64/linux/arm64/0 osx_arm64/darwin/arm64/1 windows_amd64/windows/amd64/0
for t in restic rclone; do for f in $R/work/$t.list.*.pk; do cut -f1,2 $f | grep -v -P '\t$' | tr '\t' ' ' > ${f%.pk}.raw; done
  python3 -I $R/scripts/cmp_list.py $R/bi/$t.json $R/work/$t.list; done

# 7. classifier, supplied texts, assemble, check
(cd $R/scripts/classify && . $R/scripts/env.sh 1.26.5 && go build -buildvcs=false -o $R/bin/classify .)
curl -fsSL https://www.apache.org/licenses/LICENSE-2.0.txt -o $R/dl/supplied/Apache-2.0.txt
curl -fsSL https://www.gnu.org/licenses/gpl-3.0.txt -o $R/dl/supplied/GPL-3.0.txt
mkdir -p $R/out && $R/scripts/build.sh
python3 -I $R/scripts/check.py restic
python3 -I $R/scripts/check.py rclone
```

How files are chosen: for each module, the license-like files (`LICENSE*`,
`LICENCE*`, `COPYING*`, `COPYRIGHT*`, `NOTICE*`, `PATENTS*`, `UNLICENSE*`,
`GO_LICENSE*`, `THIRD_PARTY_*`, `AUTHORS*`, `CONTRIBUTORS*`) in the module root
and in every directory between the root and a package that is actually linked
into at least one target. Files next to packages that are not linked, such as
test data, examples or unused subpackages, are left out.
`oci-go-sdk/THIRD_PARTY_LICENSES_DEV.txt` covers development-only tooling, so it
is also left out.

## Supplied texts

- `restic/supplied/Apache-2.0.txt`: `github.com/Backblaze/blazer` ships only the
  Apache-2.0 notice header as its LICENSE, so the full Apache-2.0 text is
  supplied (SHA-256
  `cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30`, from
  apache.org).
- `rclone/supplied/GPL-3.0.txt`: `github.com/cloudsoda/sddl` ships the LGPL-3.0
  text, which builds on GPL-3.0 and requires a copy of it alongside (SHA-256
  `3972dc9744f6499f0f9b2dbf76696f2ae7ad8af9b23dde66d6af86c9dfb36986`, from
  gnu.org).

## Source availability

The restic and rclone binaries are the unmodified upstream releases. Their
source is published at <https://github.com/restic/restic/tree/v0.19.0> and
<https://github.com/rclone/rclone/tree/v1.74.4>. The source of every linked
module is available at the module path and version recorded in each
`attribution.json`, including the modules under the MPL-2.0 and the
`github.com/cloudsoda/sddl` module, whose license file is the LGPL-3.0.
