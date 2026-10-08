import sys, json
# usage: verify_dl.py <bi.json> <dl.json>
bi = json.load(open(sys.argv[1]))
txt = open(sys.argv[2]).read()
dec = json.JSONDecoder(); i = 0; dl = []
while i < len(txt):
    while i < len(txt) and txt[i].isspace(): i += 1
    if i >= len(txt): break
    o, i = dec.raw_decode(txt, i); dl.append(o)
want = {}
for t in bi.values():
    for m, (v, s) in t["deps"].items(): want[(m, v)] = s
got = {(o["Path"], o["Version"]): o for o in dl}
errs = [o for o in dl if o.get("Error")]
match = mism = missing = 0
for k, s in sorted(want.items()):
    o = got.get(k)
    if not o: missing += 1; print("MISSING", k); continue
    if o.get("Sum") == s: match += 1
    else: mism += 1; print("H1 MISMATCH", k, s, o.get("Sum"))
extra = [k for k in got if k not in want]
print(f"downloaded={len(dl)} errors={len(errs)} buildinfo_mods={len(want)} h1_match={match} h1_mismatch={mism} missing={missing} extra(main)={extra}")
for k in extra: print("  main", k, got[k].get("Sum"), json.dumps(got[k].get("Origin")))
