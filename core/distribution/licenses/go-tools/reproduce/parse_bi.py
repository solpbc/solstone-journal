import sys, json, re, os
# usage: parse_bi.py <bi_dir> <tool> -> prints json {target: {path: [ver, sum]}}
bi, tool = sys.argv[1], sys.argv[2]
out = {}
for fn in sorted(os.listdir(bi)):
    if not fn.startswith(tool + "_"): continue
    tgt = fn[len(tool)+1:].removesuffix(".txt").removesuffix(".exe").replace("-", "_")
    mods = {}; settings = {}; gov = None; main = None
    for line in open(os.path.join(bi, fn)):
        if not line.startswith("\t"):
            gov = line.split(": ",1)[1].strip(); continue
        p = line.rstrip("\n").split("\t")[1:]
        if p[0] == "dep":
            assert p[1] not in mods, p
            mods[p[1]] = [p[2], p[3] if len(p) > 3 else ""]
        elif p[0] == "=>":
            raise SystemExit("replace present: %r" % p)
        elif p[0] == "mod": main = p[1:]
        elif p[0] == "build":
            k, _, v = p[1].partition("="); settings[k] = v
    out[tgt] = {"go": gov, "main": main, "settings": settings, "deps": mods}
json.dump(out, sys.stdout, indent=1, sort_keys=True)
