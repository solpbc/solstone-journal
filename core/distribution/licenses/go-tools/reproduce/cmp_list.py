import sys, json
bi = json.load(open(sys.argv[1])); pfx = sys.argv[2]; bad = 0
for t in sorted(bi):
    lst = {tuple(l.split()) for l in open(f"{pfx}.{t}.raw") if l.strip()}
    b = {(m, v[0]) for m, v in bi[t]["deps"].items()}
    bad += lst != b
    print(t, "golist", len(lst), "buildinfo", len(b), "equal" if lst == b else f"DIFF only_golist={sorted(lst-b)} only_bi={sorted(b-lst)}")
sys.exit(1 if bad else 0)
