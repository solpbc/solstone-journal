"""collect.py <tool> <gover> <main_module>: build out/<tool> from build info, go list package dirs and the module cache."""
import sys, json, os, re, shutil, subprocess
R = os.environ.get("GOLIC_ROOT", "/var/tmp/golic-r1")
tool, gover, mainmod = sys.argv[1:4]
LIC = re.compile(r'^(LICEN[CS]E|COPYING|COPYRIGHT|NOTICE|PATENTS|UNLICENSE|GO_LICENSE|THIRD[-_]PARTY[-_](NOTICES|LICENSES))([._-][^/]*)?$', re.I)
ATTR = re.compile(r'^(AUTHORS|CONTRIBUTORS)(\.[a-z]+)?$', re.I)
SKIP_EXT = re.compile(r'\.(go|sh|ya?ml|json|py|js|ts|html|tmpl|s|c|h|proto)$', re.I)
SKIP_NAME = re.compile(r'_DEV\b|_DEV\.', re.I)

def kind(n):
    u = n.upper()
    if u.startswith("NOTICE") or u.startswith("THIRD") or u.startswith("COPYING_NOTES"): return "notice"
    if u.startswith("PATENTS"): return "patents"
    if ATTR.match(n): return "authors"
    return "license"

def jsonstream(p):
    t = open(p).read(); d = json.JSONDecoder(); i = 0; out = []
    while True:
        while i < len(t) and t[i].isspace(): i += 1
        if i >= len(t): return out
        o, i = d.raw_decode(t, i); out.append(o)

bi = json.load(open(f"{R}/bi/{tool}.json"))
targets = sorted(bi)
dl = {o["Path"]: o for o in jsonstream(f"{R}/work/{tool}.dl.json")}
mods = {}
for t in targets:
    for m, (v, s) in bi[t]["deps"].items():
        mods.setdefault(m, {"version": v, "h1": s, "targets": []})["targets"].append(t)
pkgdirs = {}
for t in targets:
    for line in open(f"{R}/work/{tool}.list.{t}.pk"):
        mp, mv, ip, d = line.rstrip("\n").split("\t")
        pkgdirs.setdefault(mp, set()).add(d)

def cand_files(root, dirs):
    root = os.path.realpath(root); seen = set(); out = []
    chain = {root}
    for d in dirs:
        d = os.path.realpath(d)
        assert d == root or d.startswith(root + "/"), (root, d)
        while True:
            chain.add(d)
            if d == root: break
            d = os.path.dirname(d)
    for d in sorted(chain):
        for n in sorted(os.listdir(d)):
            p = os.path.join(d, n)
            if not os.path.isfile(p) or os.path.islink(p): continue
            if SKIP_EXT.search(n) or SKIP_NAME.search(n): continue
            if LIC.match(n) or ATTR.match(n):
                out.append(os.path.relpath(p, root))
    return out

out = f"{R}/out/{tool}"
shutil.rmtree(out, ignore_errors=True); os.makedirs(f"{out}/deps"); os.makedirs(f"{out}/go")
for n in ("LICENSE", "PATENTS"):
    shutil.copyfile(f"{R}/tc/go{gover}/go/{n}", f"{out}/go/{n}")
# main module
mroot = dl[mainmod]["Dir"]
mfiles = cand_files(mroot, pkgdirs[mainmod])
main_entry = {"module": mainmod, "version": dl[mainmod]["Version"], "h1": dl[mainmod]["Sum"],
              "origin": dl[mainmod].get("Origin"), "files": {}}
for rel in mfiles:
    dst = "LICENSE" if rel in ("LICENSE", "COPYING") else "self/" + rel
    os.makedirs(os.path.dirname(f"{out}/{dst}") or out, exist_ok=True)
    shutil.copyfile(f"{mroot}/{rel}", f"{out}/{dst}")
    main_entry["files"][dst] = rel
classify_in = []
entries = []
for m in sorted(mods):
    e = mods[m]; o = dl[m]
    assert o["Version"] == e["version"] and o["Sum"] == e["h1"], m
    rel = cand_files(o["Dir"], pkgdirs.get(m, ()))
    dname = m.replace("/", "__") + "@" + e["version"]
    files = []
    for r in rel:
        dst = f"deps/{dname}/{r}"
        os.makedirs(os.path.dirname(f"{out}/{dst}"), exist_ok=True)
        shutil.copyfile(f"{o['Dir']}/{r}", f"{out}/{dst}")
        files.append({"path": dst, "source": r, "kind": kind(os.path.basename(r))})
    entries.append({"module": m, "version": e["version"], "h1": e["h1"], "targets": e["targets"],
                    "linked_packages_found": m in pkgdirs, "files": files})
json.dump({"main": main_entry, "entries": entries, "targets": targets,
           "build": {t: {"go": bi[t]["go"], "main": bi[t]["main"], "settings": bi[t]["settings"]} for t in targets}},
          open(f"{R}/work/{tool}.collect.json", "w"), indent=1)
print(tool, "modules", len(entries), "files", sum(len(x["files"]) for x in entries), "main files", main_entry["files"])
