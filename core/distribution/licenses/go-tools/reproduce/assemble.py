"""assemble.py <tool> <gover>: write out/<tool>/attribution.json (+ supplied texts) from collect/classify outputs."""
import sys, json, os, shutil, hashlib
R = os.environ.get("GOLIC_ROOT", "/var/tmp/golic-r1"); tool, gover = sys.argv[1:3]
out = f"{R}/out/{tool}"
SPDX_OK = {"Apache-2.0", "MIT", "BSD-2-Clause", "BSD-3-Clause", "MPL-2.0", "ISC", "CC0-1.0", "UPL-1.0", "0BSD"}
RENAME = {"BSD-0-Clause": "0BSD"}
OVERRIDES = {
 "github.com/Backblaze/blazer": ("Apache-2.0", "LICENSE is only the standard Apache-2.0 notice header; the module ships no full Apache-2.0 text, so a canonical copy is supplied at supplied/Apache-2.0.txt"),
 "github.com/mattn/go-isatty": ("MIT", "file declares 'MIT License (Expat)' and carries the MIT permission text verbatim (long unwrapped lines lower classifier coverage)"),
 "github.com/willscott/go-nfs-client": ("BSD-2-Clause", "LICENSE_BSD-2.txt declares BSD-2 and carries both BSD-2 clauses (non-standard formatting); NOTICE.txt agrees"),
 "moul.io/http2curl/v2": ("Apache-2.0 OR MIT", "COPYRIGHT file: 'SPDX-License-Identifier: (Apache-2.0 OR MIT)'"),
 "github.com/sorairolake/lzip-go": ("Apache-2.0 OR MIT", "source headers 'SPDX-License-Identifier: Apache-2.0 OR MIT' and README 'either'"),
 "github.com/oracle/oci-go-sdk/v65": ("UPL-1.0 OR Apache-2.0", "LICENSE.txt: 'dual-licensed ... UPL 1.0 ... or Apache License 2.0 ... You may choose either license'; THIRD_PARTY_LICENSES.txt adds MIT and BSD-3-Clause components"),
 "github.com/ProtonMail/gluon": ("MIT", "root LICENSE is MIT; COPYING_NOTES.md restates MIT and lists dependency licenses"),
 "github.com/rclone/go-proton-api": ("MIT", "root LICENSE is MIT; COPYING_NOTES.md restates MIT and lists dependency licenses"),
 "github.com/cloudsoda/sddl": (None, "CONFLICT: LICENSE file is the full LGPL-3.0 text, but README.md says 'licensed under the MIT License - see the LICENSE file'; no SPDX headers in source. Not resolved; treat as LGPL-3.0 until upstream clarifies. LGPL-3.0 incorporates GPL-3.0, so a GPL-3.0 copy is supplied at supplied/GPL-3.0.txt"),
}
cls = {}
for l in open(f"{R}/work/{tool}.class.jsonl"):
    o = json.loads(l); cls[o["file"]] = o
def det(path):
    o = cls[path]
    return sorted({RENAME.get(m["name"], m["name"]) for m in (o["matches"] or []) if m["type"] == "License" and m["conf"] >= 0.9})
c = json.load(open(f"{R}/work/{tool}.collect.json"))
supplied = []
def supply(name):
    os.makedirs(f"{out}/supplied", exist_ok=True)
    shutil.copyfile(f"{R}/dl/supplied/{name}", f"{out}/supplied/{name}")
    supplied.append({"path": f"supplied/{name}", "sha256": hashlib.sha256(open(f"{R}/dl/supplied/{name}", "rb").read()).hexdigest(),
                     "source": {"Apache-2.0.txt": "https://www.apache.org/licenses/LICENSE-2.0.txt", "GPL-3.0.txt": "https://www.gnu.org/licenses/gpl-3.0.txt"}[name]})
mods = []
for e in c["entries"]:
    files = []
    for f in e["files"]:
        files.append({"path": f["path"], "upstream_path": f["source"], "kind": f["kind"],
                      **({"detected": det(f["path"])} if f["kind"] == "license" else {})})
    lic = [f for f in files if f["kind"] == "license"]
    alld = sorted({n for f in lic for n in f["detected"]})
    if e["module"] in OVERRIDES:
        spdx, basis = OVERRIDES[e["module"]]
    elif lic and all(f["detected"] for f in lic) and set(alld) <= SPDX_OK:
        spdx = " AND ".join(alld); basis = "licenseclassifier/v2 (conf>=0.9) over all shipped license files" + (" ; multiple licenses cover different parts of the module" if len(alld) > 1 else "")
    else:
        spdx = None; basis = "not confidently identifiable: detected " + json.dumps(alld) + " (non-SPDX classifier name or unclassified file)"
    if e["module"] == "github.com/Backblaze/blazer": supply("Apache-2.0.txt")
    if e["module"] == "github.com/cloudsoda/sddl": supply("GPL-3.0.txt")
    mods.append({"module": e["module"], "version": e["version"], "h1": e["h1"], "targets": e["targets"],
                 "license_files": [f["path"] for f in lic],
                 "notice_files": [f["path"] for f in files if f["kind"] == "notice"],
                 "other_files": [f["path"] for f in files if f["kind"] not in ("license", "notice")],
                 "detected_licenses": alld, "spdx": spdx, "spdx_basis": basis, "files": files})
mods.sort(key=lambda m: m["module"])
m = c["main"]
main_files = [{"path": k, "upstream_path": v, "detected": det(k)} for k, v in sorted(m["files"].items())]
MAIN_NOTES = {
 "restic": ["internal/fs/ea_windows.go (windows target only) embeds code copied from github.com/microsoft/go-winio pipe.go under MIT, with the MIT text inline in the source; same holder and license as deps/github.com__Microsoft__go-winio@v0.6.2/LICENSE.",
            "restic's build info reports main module version '(devel)' with no vcs stamp; the module set was reconciled by `go list -deps` on the v0.19.0 tag source (commit in origin.Hash) and matched all 4 targets exactly."],
 "rclone": ["self/cmd/bisync/LICENSE.cjnaz and self/cmd/serve/dlna/LICENSE.anacrolix are third-party licenses for code embedded in rclone's own tree (linked in all targets).",
            "backend/onedrive/quickxorhash/quickxorhash.go carries an inline 0BSD notice (namazso); 0BSD requires no attribution.",
            "linux_amd64 and linux_arm64 build info report vcs.modified=true (version v1.74.4+dirty); the module set still matches `go list -deps` on the clean v1.74.4 tag exactly, but the uncommitted change itself is not visible from the binary."],
}
doc = {
 "tool": tool,
 "main_module": {"module": m["module"], "version": m["version"], "h1": m["h1"], "origin": m["origin"], "files": main_files, "notes": MAIN_NOTES[tool]},
 "go_toolchain": {"version": "go" + gover, "files": ["go/LICENSE", "go/PATENTS"], "spdx": "BSD-3-Clause",
                   "source": f"https://go.dev/dl/go{gover}.linux-amd64.tar.gz (go/LICENSE, go/PATENTS)"},
 "targets": {t: {"go": b["go"], "main": b["main"], "settings": b["settings"]} for t, b in c["build"].items()},
 "supplied_texts": supplied,
 "module_count": len(mods),
 "modules": mods,
}
json.dump(doc, open(f"{out}/attribution.json", "w"), indent=2, sort_keys=False)
from collections import Counter
print(tool, "modules", len(mods), "spdx null:", [x["module"] for x in mods if not x["spdx"]])
print(" spdx tally:", Counter(x["spdx"] for x in mods).most_common())
