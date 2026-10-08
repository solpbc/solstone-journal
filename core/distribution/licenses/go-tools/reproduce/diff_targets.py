import sys, json
d = json.load(open(sys.argv[1]))
tg = sorted(d)
union = {}
for t in tg:
    for m, vs in d[t]["deps"].items():
        if m in union and union[m] != vs:
            print("VERSION CONFLICT", m, union[m], vs, t)
        union.setdefault(m, vs)
print("targets:", {t: len(d[t]["deps"]) for t in tg}, "union:", len(union))
print("empty sums:", [m for m, v in union.items() if not v[1]])
for m in sorted(union):
    inn = [t for t in tg if m in d[t]["deps"]]
    if len(inn) != len(tg): print("  partial:", m, union[m][0], "only in", inn)
