import sys, json, os, hashlib, filecmp
R = os.environ.get("GOLIC_ROOT", "/var/tmp/golic-r1"); tool=sys.argv[1]
a=json.load(open(f"{R}/out/{tool}/attribution.json")); bi=json.load(open(f"{R}/bi/{tool}.json"))
u={}
for t,b in bi.items():
    for m,(v,s) in b["deps"].items(): u.setdefault((m,v,s),[]).append(t)
got={(x["module"],x["version"],x["h1"]):x["targets"] for x in a["modules"]}
print("module set == build-info union (path,version,h1,targets):", got=={k:sorted(v) for k,v in u.items()}, len(got), len(u))
print("sorted by module path:", [x["module"] for x in a["modules"]]==sorted(x["module"] for x in a["modules"]))
ref=set()
for x in a["modules"]:
    for f in x["files"]: ref.add(f["path"])
ref|={f["path"] for f in a["main_module"]["files"]}|set(a["go_toolchain"]["files"])|{s["path"] for s in a["supplied_texts"]}
tree=set()
for dp,dn,fn in os.walk(f"{R}/out/{tool}"):
    for n in fn: tree.add(os.path.relpath(os.path.join(dp,n),f"{R}/out/{tool}"))
tree.discard("attribution.json")
print("tree files == referenced files:", tree==ref, len(tree), "unref:", sorted(tree-ref)[:5], "missing:", sorted(ref-tree)[:5])
# byte-identity with module cache
def js(p):
    t=open(p).read(); d=json.JSONDecoder(); i=0; o=[]
    while True:
        while i<len(t) and t[i].isspace(): i+=1
        if i>=len(t): return o
        x,i=d.raw_decode(t,i); o.append(x)
dirs={o["Path"]:o["Dir"] for o in js(f"{R}/work/{tool}.dl.json")}
bad=0
for x in a["modules"]:
    for f in x["files"]:
        if not filecmp.cmp(f"{R}/out/{tool}/{f['path']}", f"{dirs[x['module']]}/{f['upstream_path']}", shallow=False): bad+=1
for f in a["main_module"]["files"]:
    if not filecmp.cmp(f"{R}/out/{tool}/{f['path']}", f"{dirs[a['main_module']['module']]}/{f['upstream_path']}", shallow=False): bad+=1
print("byte mismatches vs module cache:", bad)
k=lambda kind: sum(1 for x in a["modules"] for f in x["files"] if f["kind"]==kind)
print(f"{tool}: modules={len(a['modules'])} dep_license_files={k('license')} dep_notice_files={k('notice')} dep_patents={k('patents')} dep_authors={k('authors')} main_files={len(a['main_module']['files'])} go_files=2 supplied={len(a['supplied_texts'])} total_files={len(tree)}")
print("main files:", [(f["path"],f["detected"]) for f in a["main_module"]["files"]])
