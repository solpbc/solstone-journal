import sys, json, os, re
R = os.environ.get("GOLIC_ROOT", "/var/tmp/golic-r1"); tool=sys.argv[1]
c=json.load(open(f"{R}/work/{tool}.collect.json"))
def js(p):
    t=open(p).read(); d=json.JSONDecoder(); i=0; o=[]
    while True:
        while i<len(t) and t[i].isspace(): i+=1
        if i>=len(t): return o
        x,i=d.raw_decode(t,i); o.append(x)
dl={o["Path"]:o for o in js(f"{R}/work/{tool}.dl.json")}
pat=re.compile(r'licen[cs]|copying|notice|copyright|patent|third.?party|authors', re.I)
skip=re.compile(r'\.(go|sh|ya?ml|json|py|js|ts|html|tmpl|proto)$', re.I)
for e in [{"module":c["main"]["module"],"files":[{"source":v} for v in c["main"]["files"].values()]}]+c["entries"]:
    root=dl[e["module"]]["Dir"]; have={f["source"] for f in e["files"]}
    for dp,dn,fn in os.walk(root):
        for n in fn:
            r=os.path.relpath(os.path.join(dp,n),root)
            if pat.search(n) and not skip.search(n) and r not in have:
                print(e["module"], r)
