#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Cold Android/iOS native artifact build and caller-link qualification."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import time
import tomllib

from android_cross_build import acquire, sha256

ROOT = Path(__file__).resolve().parents[1]
INPUTS = ROOT / "core/distribution/mobile/inputs.json"
COMPONENTS = ("rust", "llama", "parakeet", "ced", "rfdetr", "restic", "rclone", "nvattest", "pdfium", "swift")


def tree_digest(root):
    digest = hashlib.sha256()
    for path in sorted(root.rglob("*")):
        if ".git" in path.relative_to(root).parts or not path.is_file():
            continue
        digest.update(path.relative_to(root).as_posix().encode() + b"\0")
        digest.update(bytes.fromhex(sha256(path)))
    return digest.hexdigest()


def source_digest(root):
    digest = hashlib.sha256()
    files = subprocess.check_output(["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"], cwd=root)
    for name in sorted(set(files.split(b"\0")) - {b""}):
        path = root / os.fsdecode(name)
        digest.update(name + b"\0")
        if path.is_symlink():
            digest.update(b"symlink\0" + os.fsencode(os.readlink(path)))
        elif path.is_file():
            digest.update(b"file\0" + bytes.fromhex(sha256(path)))
        else:
            digest.update(b"missing\0")
    return digest.hexdigest()


def require_android_architecture(text):
    machines = re.findall(r"^\s*Machine:\s*(.+)$", text, re.MULTILINE)
    if not machines or any(machine.strip() != "AArch64" for machine in machines):
        raise ValueError(f"unexpected ELF machines: {machines}")


def require_ios_platform(text):
    platforms = re.findall(r"^\s*platform\s+(\S+)", text, re.MULTILINE)
    legacy = re.findall(r"^\s*cmd\s+LC_VERSION_MIN_(\S+)", text, re.MULTILINE)
    if not platforms and not legacy:
        raise ValueError("missing iOS device platform")
    if any(value not in ("2", "IOS") for value in platforms) or any(value != "IPHONEOS" for value in legacy):
        raise ValueError(f"unexpected Apple platforms: {platforms}, {legacy}")
    minimums = re.findall(r"^\s*minos\s+(\d+(?:\.\d+)+)", text, re.MULTILINE)
    for block in re.findall(r"cmd\s+LC_VERSION_MIN_IPHONEOS\b(.*?)(?=^Load command|\Z)", text, re.MULTILINE | re.DOTALL):
        minimums += re.findall(r"^\s*version\s+(\d+(?:\.\d+)+)", block, re.MULTILINE)
    if any(tuple(map(int, value.split("."))) > (26, 0, 0) for value in minimums):
        raise ValueError(f"artifact requires newer than iOS 26: {minimums}")


def require_current_pins(pins):
    distribution = ROOT / "core/crates/solstone-core-distribution/src"
    guards = (("llama", "llama_windows_source.rs", "LLAMA_COMMIT"),
              ("ced", "ced_windows.rs", "CED_CPP_COMMIT"),
              ("ggml", "ced_windows.rs", "GGML_COMMIT"),
              ("rfdetr", "rfdetr_windows.rs", "RF_DETR_COMMIT"),
              ("ggml", "rfdetr_windows.rs", "GGML_COMMIT"),
              ("nvattest", "nvattest_windows.rs", "NVATTEST_SDK_REVISION"),
              ("pdfium-android", "pdfium.rs", "RELEASE_TAG"),
              ("pdfium-ios", "pdfium.rs", "RELEASE_TAG"))
    for name, filename, constant in guards:
        matches = re.findall(rf'^pub const {constant}: &str = "([^\"]+)";',
                             (distribution / filename).read_text(), re.MULTILINE)
        if matches != [pins[name]["revision"]]:
            raise ValueError(f"mobile input must follow {filename}:{constant}: {matches}")
    catalog = (ROOT / "core/crates/solstone-core-assets/src/lib.rs").read_text()
    records = re.findall(r"Artifact\s*\{(.*?)\n\s*\},", catalog, re.DOTALL)
    for name, expected in (("restic", "0.19.0"), ("rclone", "1.74.4"), ("parakeet-server", "v0.6.1")):
        versions = set()
        for record in records:
            if f'unit: "{name}"' not in record:
                continue
            if name == "parakeet-server" and "-linux-" not in record:
                continue
            versions.update(re.findall(r'version: "([^\"]+)"', record))
        if versions != {expected}:
            raise ValueError(f"mobile recipe must follow current {name} catalog: {versions}")
    for name in ("restic", "rclone"):
        if pins[name]["revision"] != "v" + {"restic": "0.19.0", "rclone": "1.74.4"}[name]:
            raise ValueError(f"mobile {name} source differs from catalog version")


class Build:
    def __init__(self, args):
        self.args = args
        self.work = args.work_dir.resolve()
        # A fresh destination rules out stale sources, objects and success receipts.
        self.work.mkdir(parents=True, exist_ok=False)
        self.cache = args.cache_dir.resolve()
        self.cache.mkdir(parents=True, exist_ok=True)
        self.receipts = self.work / "receipts"
        self.receipts.mkdir()
        self.sources = self.work / "sources"
        self.sources.mkdir()
        self.pins = json.loads(INPUTS.read_text())["inputs"]
        self.env = os.environ.copy()
        for key in ("IPHONEOS_DEPLOYMENT_TARGET", "MACOSX_DEPLOYMENT_TARGET", "SDKROOT", "GOTOOLCHAIN", "GOROOT", "GOFLAGS", "CGO_CFLAGS", "CGO_LDFLAGS"):
            self.env.pop(key, None)
        self.env.update(TMPDIR=str(self.work), CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS=str(args.jobs),
                        CARGO_TARGET_DIR=str(self.work / "rust-target"))
        self.summary = {"platform": args.platform, "scope": args.components, "complete": False,
                        "status": "running", "exit": None,
                        "source_commit": self.output(["git", "rev-parse", "HEAD"], ROOT),
                        "source_clean": not self.output(["git", "status", "--porcelain"], ROOT),
                        "source_diff_sha256": hashlib.sha256(subprocess.check_output(["git", "diff", "HEAD", "--binary"], cwd=ROOT)).hexdigest(),
                        "source_tree_sha256": source_digest(ROOT),
                        "driver_sha256": sha256(Path(__file__)), "inputs_sha256": sha256(INPUTS),
                        "inputs": {}, "steps": [], "artifacts": [], "patches": [], "configurations": {},
                        "boundary": "compile and caller linkage; no device runtime, model inference, packaging or signing"}
        self.save()

    def output(self, argv, cwd=None):
        return subprocess.check_output(list(map(str, argv)), cwd=cwd, env=self.env, text=True).strip()

    def run(self, name, argv, cwd=ROOT, env=None):
        log = self.receipts / f"{name}.log"
        started = time.time()
        print(f"{name}: {' '.join(map(str, argv))}", flush=True)
        with log.open("wb") as stream:
            result = subprocess.run(list(map(str, argv)), cwd=cwd, env=env or self.env,
                                    stdout=stream, stderr=subprocess.STDOUT)
            code = result.returncode
        receipt = {"name": name, "argv": list(map(str, argv)), "cwd": str(cwd),
                   "started_epoch": started, "seconds": round(time.time() - started, 3),
                   "exit": code, "log": str(log), "log_sha256": sha256(log)}
        self.summary["steps"].append(receipt)
        (self.receipts / f"{name}.json").write_text(json.dumps(receipt, indent=2) + "\n")
        self.save()
        if code:
            print(log.read_text(errors="replace")[-12000:], file=sys.stderr)
            raise subprocess.CalledProcessError(code, argv)
        return log

    def save(self, code=None):
        if code is not None:
            self.summary.update(exit=code, status="passed" if code == 0 else "failed")
        (self.receipts / "mobile-native-build.json").write_text(json.dumps(self.summary, indent=2) + "\n")

    def source(self, name, destination=None):
        pin = dict(self.pins[name])
        pin["filename"] = f"{name}-{pin['sha256']}.tar.gz"
        archive = acquire(pin, self.cache)
        destination = destination or self.sources / name
        if destination.exists() and any(destination.iterdir()):
            raise ValueError(f"source destination is populated: {destination}")
        destination.mkdir(parents=True, exist_ok=True)
        with tarfile.open(archive) as bundle:
            for member in bundle.getmembers():
                if pin.get("strip_root", True):
                    if "/" not in member.name:
                        continue
                    member.name = member.name.split("/", 1)[1]
                if member.name:
                    bundle.extract(member, destination, filter="data")
        self.summary["inputs"][name] = {**pin, "archive": str(archive), "source": str(destination)}
        self.save()
        return destination

    def setup(self):
        require_current_pins(self.pins)
        ios = self.args.platform == "ios"
        expected = ("Darwin", "arm64") if ios else ("Linux", "x86_64")
        if (platform.system(), platform.machine()) != expected:
            raise ValueError(f"{self.args.platform} requires {expected[0]}/{expected[1]}")
        self.target = "aarch64-apple-ios" if ios else "aarch64-linux-android"
        self.run("rust-version", ["rustc", "-vV"])
        self.run("cmake-version", ["cmake", "--version"])
        self.run("rust-target", ["rustup", "target", "add", self.target])
        self.common = ["-DCMAKE_BUILD_TYPE=Release", "-DBUILD_SHARED_LIBS=OFF", "-DCMAKE_POSITION_INDEPENDENT_CODE=ON", "-DGGML_NATIVE=OFF", "-DGGML_OPENMP=OFF", "-DGGML_CCACHE=OFF", "-DGGML_LLAMAFILE=OFF", "-DGGML_BACKEND_DL=OFF"]
        self.native = self.work / "workspace-native"
        if ios:
            self.sdk = self.output(["xcrun", "--sdk", "iphoneos", "--show-sdk-path"])
            self.cc = self.output(["xcrun", "--sdk", "iphoneos", "--find", "clang"])
            self.cxx = self.output(["xcrun", "--sdk", "iphoneos", "--find", "clang++"])
            self.cc_flags = ["-arch", "arm64", "-isysroot", self.sdk, "-miphoneos-version-min=26.0"]
            self.common += ["-DCMAKE_SYSTEM_NAME=iOS", f"-DCMAKE_OSX_SYSROOT={self.sdk}", "-DCMAKE_OSX_ARCHITECTURES=arm64", "-DCMAKE_OSX_DEPLOYMENT_TARGET=26.0", "-DCMAKE_MACOSX_BUNDLE=OFF", "-DGGML_METAL=ON", "-DGGML_METAL_EMBED_LIBRARY=ON"]
            self.run("xcode-version", ["xcodebuild", "-version"])
        else:
            # Toolchain acquisition is shared with the workspace producer.
            from android_cross_build import NDK
            archive = acquire(NDK, self.cache)
            self.run("ndk-extract", ["unzip", "-q", "-DD", archive, "-d", self.work])
            self.ndk = self.work / "android-ndk-r30"
            tools = self.ndk / "toolchains/llvm/prebuilt/linux-x86_64/bin"
            self.cc = str(tools / "aarch64-linux-android31-clang")
            self.cxx = str(tools / "aarch64-linux-android31-clang++")
            self.cc_flags = []
            self.env["PATH"] = str(tools) + os.pathsep + self.env["PATH"]
            self.env["ANDROID_NDK_ROOT"] = str(self.ndk)
            self.common += [f"-DCMAKE_TOOLCHAIN_FILE={self.ndk}/build/cmake/android.toolchain.cmake", "-DANDROID_ABI=arm64-v8a", "-DANDROID_PLATFORM=android-31", "-DANDROID_STL=c++_static", "-DGGML_CUDA=OFF", "-DGGML_VULKAN=OFF", "-DGGML_METAL=OFF", "-DGGML_BLAS=OFF", "-DGGML_OPENCL=OFF"]
        self.run("clang-version", [self.cc, "--version"])

    def cmake(self, name, source, flags, targets=None, generator="Unix Makefiles", env=None):
        build = self.work / "build" / name
        self.run(f"{name}-configure", ["cmake", "-G", generator, "-S", source, "-B", build, *self.common, *flags], env=env)
        cache = self.receipts / f"{name}-CMakeCache.txt"
        shutil.copyfile(build / "CMakeCache.txt", cache)
        self.summary["configurations"][name] = {"cache": str(cache), "sha256": sha256(cache)}
        self.save()
        command = ["cmake", "--build", build, f"-j{self.args.jobs}"]
        if targets:
            command += ["--target", *targets]
        self.run(f"{name}-build", command, env=env)
        return build

    def inspect(self, name, path):
        if not path.is_file() or not path.stat().st_size:
            raise ValueError(f"missing artifact: {path}")
        if self.args.platform == "ios":
            text = self.run(f"{name}-architecture", ["lipo", "-archs", path]).read_text().strip()
            if text != "arm64":
                raise ValueError(f"unexpected architectures {text}: {path}")
            text = self.run(f"{name}-platform", ["otool", "-l", path]).read_text()
            require_ios_platform(text)
        else:
            tools = self.ndk / "toolchains/llvm/prebuilt/linux-x86_64/bin"
            text = self.run(f"{name}-architecture", [tools / "llvm-readelf", "-h", path]).read_text()
            require_android_architecture(text)
        self.summary["artifacts"].append({"component": name, "path": str(path), "sha256": sha256(path), "size_bytes": path.stat().st_size})

    def link(self, name, code, includes, libraries, flags=()):
        folder = self.work / "artifacts" / name
        folder.mkdir(parents=True, exist_ok=True)
        caller = folder / "caller.c"
        caller.write_text(code)
        probe = folder / "caller-link"
        command = [self.cxx, *self.cc_flags, "-x", "c", caller, "-x", "none", *[f"-I{x}" for x in includes]]
        if self.args.platform == "android":
            command += ["-Wl,--start-group", *libraries, "-Wl,--end-group", "-llog", "-ldl", "-lm", "-latomic"]
        else:
            command += [*libraries, "-framework", "Metal", "-framework", "MetalKit", "-framework", "Foundation", "-framework", "Accelerate"]
        self.run(f"{name}-caller-link", [*command, *flags, "-o", probe])
        self.inspect(f"{name}-caller", probe)

    def ggml(self, name, source):
        ggml = self.source("ggml", source / "third_party/ggml")
        self.run(f"{name}-ggml-git", ["git", "init", ggml])
        if name in ("parakeet", "rfdetr"):
            script = source / "scripts/apply_ggml_patches.sh"
            self.run(f"{name}-patch-series", ["bash", script], source)
            patches = sorted((source / "third_party/ggml-patches").glob("*.patch"))
            if not patches:
                raise ValueError(f"GGML patch series missing: {source}")
            for i, patch in enumerate(patches):
                self.run(f"{name}-patch-{i}-proof", ["git", "apply", "--check", "--reverse", patch], ggml)
                self.summary["patches"].append({"component": name, "path": str(patch.relative_to(source)), "sha256": sha256(patch)})
            self.summary["inputs"][f"{name}-patched-ggml"] = {"tree_sha256": tree_digest(ggml)}

    def openssl(self):
        source = self.source("openssl")
        prefix = self.work / "openssl-install"
        env = self.env.copy()
        if self.args.platform == "ios":
            env.update(SDKROOT=self.sdk)
            target = "ios64-xcrun"
            flags = " ".join(self.cc_flags) + " -fPIC"
        else:
            target = "android-arm64"
            flags = "-fPIC"
            env["CPPFLAGS"] = "-D__ANDROID_API__=31"
        self.run("openssl-configure", ["perl", "Configure", target, "no-shared", "no-tests", "no-apps", "no-docs", f"--prefix={prefix}", "--libdir=lib", f"CC={self.cc}", f"CFLAGS={flags}"], source, env)
        self.run("openssl-build", ["make", f"-j{self.args.jobs}"], source, env)
        self.run("openssl-install", ["make", "install_dev"], source, env)
        self.inspect("openssl-crypto", prefix / "lib/libcrypto.a")
        self.inspect("openssl-ssl", prefix / "lib/libssl.a")
        return prefix

    def cpp(self, name):
        source = self.source(name)
        flags = []
        if name == "llama":
            openssl = self.openssl()
            flags = ["-DLLAMA_BUILD_TESTS=OFF", "-DLLAMA_BUILD_EXAMPLES=OFF", "-DLLAMA_BUILD_SERVER=ON", "-DLLAMA_BUILD_TOOLS=ON", "-DLLAMA_OPENSSL=ON", "-DLLAMA_BUILD_NUMBER=11429", "-DLLAMA_BUILD_COMMIT=d812350", f"-DOPENSSL_ROOT_DIR={openssl}", f"-DOPENSSL_INCLUDE_DIR={openssl}/include", f"-DOPENSSL_CRYPTO_LIBRARY={openssl}/lib/libcrypto.a", f"-DOPENSSL_SSL_LIBRARY={openssl}/lib/libssl.a", "-DOPENSSL_USE_STATIC_LIBS=TRUE"]
            header = "llama.h"
            code = '#include "llama.h"\nint main(void){llama_backend_init(); const char *s=llama_print_system_info(); llama_backend_free(); return s == 0;}\n'
        else:
            if name == "parakeet":
                self.source("parakeet-ced", source / "third_party/ced.cpp")
                self.source("parakeet-voice-detect", source / "third_party/voice-detect.cpp")
            self.ggml(name, source)
            prefix = name.upper()
            flags = [f"-D{prefix}_BUILD_TESTS=OFF", f"-D{prefix}_BUILD_CLI=ON", f"-D{prefix}_SHARED=OFF", f"-D{prefix}_GGML_METAL={'ON' if self.args.platform == 'ios' else 'OFF'}"]
            if name == "parakeet":
                flags += ["-DPARAKEET_WITH_CED=ON", "-DPARAKEET_WITH_VOICEDETECT=ON", f"-DPARAKEET_BUILD_SERVER={'OFF' if self.args.platform == 'ios' else 'ON'}"]
            header = f"{name}_capi.h"
            if name == "rfdetr":
                code = '#include "rfdetr_capi.h"\nint main(void){rfdetr_handle_t h; int rc=rfdetr_capi_load("/missing.gguf",1,&h); if(!rc)rfdetr_capi_unload(h); return rc;}\n'
            else:
                code = f'#include "{header}"\nint main(void){{ {name}_ctx *c={name}_capi_load("/missing.gguf"); if(c){name}_capi_free(c); return {name}_capi_abi_version(); }}\n'
                if name == "ced":
                    code = (ROOT / "core/distribution/mobile/probes/ced.c").read_text()
        build = self.cmake(name, source, flags)
        headers = list(source.rglob(header))
        if len(headers) != 1:
            raise ValueError(f"expected one {header}: {headers}")
        libraries = sorted(build.rglob("*.a"))
        if not any(x.name == f"lib{name}.a" for x in libraries):
            raise ValueError(f"missing {name} library")
        for i, library in enumerate(libraries):
            self.inspect(f"{name}-archive-{i}", library)
        for executable in sorted((build / "bin").glob("*")):
            if executable.is_file() and executable.stat().st_mode & 0o111:
                self.inspect(f"{name}-{executable.name}", executable)
        self.link(name, code, [headers[0].parent, source / "ggml/include", source / "third_party/ggml/include"], libraries)

    def pdfium(self):
        source = self.source(f"pdfium-{self.args.platform}")
        library = source / "lib" / ("libpdfium.dylib" if self.args.platform == "ios" else "libpdfium.so")
        self.inspect("pdfium-library", library)
        self.link("pdfium", (ROOT / "core/distribution/mobile/probes/pdfium.c").read_text(), [source / "include"], [library])

    def go(self, name):
        tool = self.work / "go-toolchain"
        if not tool.exists():
            self.source(f"go-{self.args.platform}", tool)
        go = tool / "bin/go"
        source = self.source(name)
        env = self.env.copy()
        env.update(GOROOT=str(tool), GOTOOLCHAIN="local", GOPATH=str(self.work / "gopath"),
                   GOMODCACHE=str(self.work / "gomodcache"), GOCACHE=str(self.work / "gocache"),
                   GOOS=self.args.platform, GOARCH="arm64", CGO_ENABLED="1", CC=self.cc, CXX=self.cxx,
                   CGO_CFLAGS=" ".join(self.cc_flags), CGO_LDFLAGS=" ".join(self.cc_flags),
                   GOPROXY="https://proxy.golang.org", GOSUMDB="sum.golang.org", GOMAXPROCS=str(self.args.jobs))
        for key in ("GOPRIVATE", "GONOPROXY", "GONOSUMDB", "GOINSECURE", "GOWORK"):
            env.pop(key, None)
        env["GOWORK"] = "off"
        self.run(f"{name}-go-version", [go, "version"], source, env)
        self.run(f"{name}-modules", [go, "mod", "download", "-modcacherw"], source, env)
        self.run(f"{name}-module-verify", [go, "mod", "verify"], source, env)
        self.summary["inputs"][f"{name}-go-locks"] = {x: sha256(source / x) for x in ("go.mod", "go.sum")}
        if name == "rclone" and self.args.platform == "ios":
            listing = self.run("rclone-storj-coordinate", [go, "list", "-m", "-json", "storj.io/common"], source, env)
            module = json.loads(listing.read_text())
            if module["Version"] != "v0.0.0-20260225132117-99155641c30a":
                raise ValueError("Storj patch coordinate drifted")
            local = self.work / "storj-common"
            shutil.copytree(module["Dir"], local)
            path = local / "internal/hmacsha512/cpu_darwin_arm64.go"
            original = sha256(path)
            if original != "fb80f2cab5f5c78d3f2b2d2b4784c7a745263ecaba58ba9f30000ed496ffde64":
                raise ValueError("Storj source file drifted")
            text = path.read_text()
            if "//go:build" in text:
                raise ValueError("Storj build constraint drifted")
            path.chmod(path.stat().st_mode | 0o200)
            path.write_text("//go:build !ios\n\n" + text)
            self.run("rclone-storj-replace", [go, "mod", "edit", f"-replace=storj.io/common={local}"], source, env)
            self.summary["patches"].append({"component": "rclone-storj", "file": "internal/hmacsha512/cpu_darwin_arm64.go", "original_sha256": original, "patched_sha256": sha256(path), "change": "add //go:build !ios"})
            self.summary["inputs"]["rclone-patched-storj"] = {"tree_sha256": tree_digest(local)}
        env.update(GOPROXY="off", GOSUMDB="off")
        out = self.work / "artifacts" / name
        out.mkdir(parents=True)
        common = [go, "build", "-p", str(self.args.jobs), "-trimpath", "-mod=readonly", "-buildvcs=false"]
        command = [*common, "-ldflags=-s -w", "-o", out / name]
        if self.args.platform == "android":
            command += ["-buildmode=pie"]
        self.run(f"{name}-cli", [*command, "./cmd/restic" if name == "restic" else "."], source, env)
        self.inspect(f"{name}-cli", out / name)
        if self.args.platform == "ios":
            archive = out / f"lib{name}.a"
            if name == "rclone":
                package = "./librclone"
                code = '#include "librclone.h"\nint main(void){RcloneInitialize(); struct RcloneRPC_return r=RcloneRPC("core/version","{}"); RcloneFreeString(r.r0); RcloneFinalize(); return r.r1;}\n'
            else:
                probe = source / "internal_mobile_link"
                probe.mkdir()
                (probe / "main.go").write_text('package main\n/*\n#include <stdlib.h>\n*/\nimport "C"\nimport "github.com/restic/restic/internal/repository"\n//export ResticRepositoryLink\nfunc ResticRepositoryLink() C.int { r, e := repository.New(nil, repository.Options{}); if e != nil { return 1 }; return C.int(r.PackSize()) }\nfunc main() {}\n')
                package = "./internal_mobile_link"
                code = '#include "librestic.h"\nint main(void){return ResticRepositoryLink();}\n'
            self.run(f"{name}-c-archive", [*common, "-buildmode=c-archive", "-o", archive, package], source, env)
            self.inspect(f"{name}-c-archive", archive)
            self.link(name, code, [out], [archive], ["-framework", "CoreFoundation", "-framework", "Security", "-lresolv"])

    def restic(self):
        self.go("restic")

    def rclone(self):
        self.go("rclone")

    def nvattest(self):
        source = self.source("nvattest")
        patch = ROOT / "core/distribution/mobile/nvattest-mobile.patch"
        self.run("nvattest-patch-check", ["git", "apply", "--check", patch], source)
        self.run("nvattest-patch", ["git", "apply", patch], source)
        self.run("nvattest-patch-proof", ["git", "apply", "--check", "--reverse", patch], source)
        self.summary["patches"].append({"component": "nvattest", "sha256": sha256(patch)})
        corrosion = self.source("nvattest-corrosion")
        regorus = self.source("nvattest-regorus")
        script = regorus / "build.rs"
        original = sha256(script)
        if original != "7dc931d2a3cc9203b9cf63c9da29e85122f8b10ae2b1a4318494eacc178a3956":
            raise ValueError("Regorus revision build script drifted")
        old = '''        let output = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("`git rev-parse HEAD` failed.");
        let git_hash = String::from_utf8(output.stdout).unwrap();'''
        text = script.read_text()
        if text.count(old) != 1:
            raise ValueError("Regorus revision substitution drifted")
        revision = self.pins["nvattest-regorus"]["url"].rsplit("/", 1)[1]
        if not re.fullmatch(r"[0-9a-f]{40}", revision):
            raise ValueError("Regorus requires an immutable source revision")
        script.write_text(text.replace(old, f'        let git_hash = "{revision}";'))
        self.summary["patches"].append({"component": "regorus-build-revision", "revision": revision,
                                       "original_sha256": original, "patched_sha256": sha256(script)})
        env = self.env.copy()
        env.update(NVAT_SOURCE_COMMIT=self.pins["nvattest"]["revision"],
                   CMAKE_BUILD_PARALLEL_LEVEL=str(self.args.jobs))
        if self.args.platform == "android":
            env["CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER"] = self.cc
        flags = ["-DBUILD_SHARED_LIBS=ON", "-DUSE_SYSTEM_DEPS=OFF", "-DBUILD_TESTING=OFF", "-DBUILD_EXAMPLES=OFF", f"-DRust_CARGO_TARGET={self.target}", f"-DFETCHCONTENT_SOURCE_DIR_CORROSION={corrosion}", f"-DFETCHCONTENT_SOURCE_DIR_REGORUS={regorus}", f"-DCMAKE_BUILD_PARALLEL_LEVEL={self.args.jobs}"]
        for dependency in ("jwt-cpp", "fmt", "spdlog", "json", "cli11"):
            prepared = self.source(f"nvattest-{dependency}")
            flags += [f"-DFETCHCONTENT_SOURCE_DIR_{dependency.upper()}={prepared}"]
        # ExternalProject keeps its source-provided URL_HASH checks. Supply the
        # same verified bytes locally so the resulting receipt covers acquisition.
        cmake = source / "nv-attestation-sdk-cpp/CMakeLists.txt"
        text = cmake.read_text()
        for dependency in ("openssl", "libxml2", "xmlsec1", "curl"):
            pin = dict(self.pins[f"nvattest-{dependency}"])
            pin["filename"] = f"nvattest-{dependency}-{pin['sha256']}.tar.gz"
            archive = acquire(pin, self.cache)
            if text.count(pin["url"]) != 1:
                raise ValueError(f"SDK dependency coordinate drift: {dependency}")
            text = text.replace(pin["url"], archive.as_uri())
            self.summary["inputs"][f"nvattest-{dependency}"] = {**pin, "archive": str(archive)}
        cmake.write_text(text)
        self.summary["inputs"]["nvattest-patched-source"] = {"tree_sha256": tree_digest(source), "regorus_lock_sha256": sha256(source / "sol/release/regorus-Cargo.lock")}
        ios = self.args.platform == "ios"
        build = self.cmake("nvattest", source / ("nv-attestation-sdk-cpp" if ios else "nv-attestation-cli"), flags, ["nvat" if ios else "nvattest"], "Unix Makefiles", env)
        libdir = build if ios else build / "nv-attestation-sdk-build"
        library = libdir / ("libnvat.dylib" if ios else "libnvat.so")
        self.inspect("nvattest-library", library)
        if not ios:
            self.inspect("nvattest-cli", build / "nvattest")
        self.link("nvattest", '#include "nvat.h"\nint main(void){nvat_sdk_opts_t o=0; nvat_rc_t r=nvat_sdk_opts_create(&o); r=nvat_sdk_init(o); const char *s=nvat_rc_to_string(r); nvat_sdk_shutdown(); return s==0;}\n', [libdir / "include"], [library])
        targets = tomllib.loads((source / "sol/release/targets.toml").read_text())["release"]
        pin = {"url": targets["ca_bundle_url"], "sha256": targets["ca_bundle_sha256"],
               "filename": "nvattest-ca-bundle.pem"}
        certificate = acquire(pin, self.cache)
        destination = self.work / "artifacts/nvattest/ca-bundle.pem"
        shutil.copyfile(certificate, destination)
        self.summary["inputs"]["nvattest-ca-bundle"] = pin
        self.summary["artifacts"].append({"component": "nvattest-ca-bundle", "path": str(destination),
                                          "sha256": sha256(destination), "size_bytes": destination.stat().st_size})

    def swift(self):
        if self.args.platform != "ios":
            raise ValueError("FluidAudio is an Apple CoreML component; use Parakeet C++ on Android")
        source = ROOT / "core/crates/solstone-core-transcribe/parakeet-helper"
        staged = self.work / "parakeet-helper"
        shutil.copytree(source, staged, ignore=shutil.ignore_patterns(".build", ".swiftpm", "_bin"))
        self.run("swift-helper-resolve", ["xcodebuild", "-resolvePackageDependencies", "-scheme", "parakeet-helper", "-clonedSourcePackagesDirPath", self.work / "spm", "-onlyUsePackageVersionsFromResolvedFile"], staged)
        self.run("swift-helper-build", ["xcodebuild", "-scheme", "parakeet-helper", "-destination", "generic/platform=iOS", "-sdk", "iphoneos", "-configuration", "Release", "-clonedSourcePackagesDirPath", self.work / "spm", "-derivedDataPath", self.work / "swift-derived", "-onlyUsePackageVersionsFromResolvedFile", "CODE_SIGNING_ALLOWED=NO", "-jobs", str(self.args.jobs), "build"], staged)
        output = self.work / "swift-derived/Build/Products/Release-iphoneos/parakeet-helper"
        self.inspect("swift-helper", output)
        resolved = json.loads((staged / "Package.resolved").read_text())
        if len(resolved["pins"]) != 1 or resolved["pins"][0]["state"]["revision"] != "b10bdcb51dcfeedb8001353b4ba363da1a616d6d":
            raise ValueError("FluidAudio resolution differs from pin")
        self.summary["inputs"]["swift-lock"] = {"sha256": sha256(staged / "Package.resolved")}

    def rust(self):
        if self.args.platform == "android":
            self.run("rust-workspace", [sys.executable, ROOT / "scripts/android_cross_build.py", "--work-dir", self.native, "--cache-dir", self.cache, "--jobs", str(self.args.jobs)])
        else:
            env = self.env.copy()
            env["IOS_ONNX_RUNTIME_LIB_DIGEST"] = "4eb86d500c6994fea07f834c1fa632f1953302d30776195dc6b8e5a95800c3e3"
            ffmpeg = tomllib.loads((ROOT / "core/distribution/builder-inputs.toml").read_text())["ffmpeg"]
            env["SOLSTONE_FFMPEG_SOURCE_ARCHIVE"] = str(acquire(ffmpeg, self.cache))
            self.run("ios-onnx-prepare", ["bash", ROOT / "scripts/ios_cross_build.sh", "prepare", self.native], env=env)
            self.run("rust-workspace", ["bash", ROOT / "scripts/ios_cross_build.sh", "check", self.native], env=env)
        self.inspect("onnx-runtime", self.native / "lib" / ("libonnxruntime.a" if self.args.platform == "ios" else "libonnxruntime.so"))
        target = self.work / "rust-target" / self.target / "debug"
        for executable in sorted(target.glob("*")):
            if executable.is_file() and executable.stat().st_mode & 0o111:
                self.inspect(f"rust-{executable.name}", executable)
        native_archives = sorted((target / "build").glob("*/out/**/*.a"))
        if not any(path.name == "libavcodec.a" for path in native_archives):
            raise ValueError("workspace did not produce FFmpeg native archives")
        for index, library in enumerate(native_archives):
            self.inspect(f"rust-native-{index}", library)

    def execute(self):
        self.setup()
        for component in self.args.components:
            if component in ("llama", "parakeet", "ced", "rfdetr"):
                self.cpp(component)
            else:
                getattr(self, component)()
        self.summary["complete"] = self.args.full_run
        self.save(0)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=("android", "ios"), required=True)
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--cache-dir", type=Path, required=True)
    parser.add_argument("--jobs", type=int, default=6)
    parser.add_argument("--components", nargs="+", choices=COMPONENTS, help="subset diagnostics; never a full-stack pass")
    args = parser.parse_args()
    if args.jobs < 1:
        parser.error("--jobs must be positive")
    args.full_run = args.components is None
    if args.full_run:
        args.components = list(COMPONENTS if args.platform == "ios" else COMPONENTS[:-1])
    build = None
    try:
        build = Build(args)
        build.execute()
    except (ValueError, OSError, subprocess.CalledProcessError, AttributeError) as error:
        if build:
            build.summary["error"] = str(error)
            build.save(getattr(error, "returncode", 1))
        print(f"mobile native build failed: {error}", file=sys.stderr)
        return getattr(error, "returncode", 1)
    return 0


if __name__ == "__main__":
    sys.exit(main())
