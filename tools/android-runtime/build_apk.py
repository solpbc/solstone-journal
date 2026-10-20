#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Build the isolated operator harness using SDK tools, no product Gradle tree."""
import argparse, hashlib, json, os, pathlib, re, shutil, subprocess, time, zipfile
import xml.etree.ElementTree as ET
from provenance import source_manifest
p=argparse.ArgumentParser(); p.add_argument('--root',required=True); p.add_argument('--sdk',required=True); p.add_argument('--java',required=True); p.add_argument('--application-id', default='app.solstone.development.journal'); p.add_argument('--debug-keystore',required=True,type=pathlib.Path); args=p.parse_args()
if not re.fullmatch(r'[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)+', args.application_id): p.error('invalid application id')
key=args.debug_keystore.resolve(strict=True)
if not key.is_file(): p.error('regular retained debug keystore required')
root=pathlib.Path(args.root).resolve(); sdk=pathlib.Path(args.sdk); java=pathlib.Path(args.java); build=root/'build'; build.mkdir(exist_ok=True)
(build/'source-manifest.json').write_text(json.dumps(source_manifest(root.parents[1]),indent=2)+'\n')
tools=sdk/'build-tools/35.0.0'; jar=sdk/'platforms/android-36/android.jar'; commands=[]
def run(argv):
  start=time.time(); out=subprocess.run([str(a) for a in argv],stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,timeout=120)
  commands.append({'argv':[str(a) for a in argv],'exit':out.returncode,'elapsed_seconds':time.time()-start,'output':out.stdout.decode(errors='replace')})
  (build/'build-receipts.json').write_text(json.dumps(commands,indent=2))
  if out.returncode: raise RuntimeError(commands[-1])
classes=build/'classes'; dex=build/'dex'; host_classes=build/'host-tests'
for generated in [classes,dex,host_classes]:
  if generated.exists():shutil.rmtree(generated)
  generated.mkdir()
owner=root/'src/app/solstone/development/journal'
run([java/'bin/javac','-d',host_classes,owner/'ThinkingAdmission.java',owner/'JournalExecution.java',owner/'ServingConnection.java',owner/'PackagedPayload.java',root/'tests/ThinkingAdmissionTest.java',root/'tests/PackagedPayloadTest.java'])
run([java/'bin/java','-cp',host_classes,'app.solstone.development.journal.ThinkingAdmissionTest'])
run([java/'bin/java','-cp',host_classes,'app.solstone.development.journal.PackagedPayloadTest'])
run([java/'bin/javac','-source','8','-target','8','-cp',jar,'-d',classes,*sorted((root/'src').rglob('*.java'))])
env=os.environ.copy(); env['JAVA_HOME']=str(java); env['PATH']=str(java/'bin')+':'+env.get('PATH',''); os.environ.update(env)
run([tools/'d8','--lib',jar,'--min-api','31','--output',dex,*sorted(classes.rglob('*.class'))])
manifest=ET.parse(root/'AndroidManifest.xml'); original=manifest.getroot().attrib['package']
manifest.getroot().set('package',args.application_id)
for element in manifest.iter():
  name=element.get('{http://schemas.android.com/apk/res/android}name')
  if name and name.startswith('.'): element.set('{http://schemas.android.com/apk/res/android}name',original+name)
manifest_path=build/'AndroidManifest.xml'; manifest.write(manifest_path,encoding='utf-8',xml_declaration=True)
unsigned=build/'unsigned.apk'
run([tools/'aapt2','link','-I',jar,'--manifest',manifest_path,'--min-sdk-version','31','--target-sdk-version','36','--version-code','1','--version-name','development-1','-o',unsigned])
with zipfile.ZipFile(unsigned,'a',compression=zipfile.ZIP_DEFLATED) as z:
  z.write(dex/'classes.dex','classes.dex')
  for native in sorted((root/'lib').rglob('*.so')): z.write(native,str(native.relative_to(root)))
  for asset in sorted((root/'assets').rglob('*')):
    if asset.is_file(): z.write(asset,str(asset.relative_to(root)))
  payload=root.parents[1]/'core/payload'
  for asset in sorted((payload/'solstone').rglob('*')):
    if asset.is_file(): z.write(asset,'assets/journal-payload/'+str(asset.relative_to(payload)))
aligned=build/'aligned.apk'; run([tools/'zipalign','-P','16','-f','4',unsigned,aligned])
signed=build/'journal-process-runtime.apk'
run([tools/'apksigner','sign','--ks',key,'--ks-pass','pass:android','--key-pass','pass:android','--ks-key-alias','androiddebugkey','--out',signed,aligned])
run([tools/'apksigner','verify','--verbose','--print-certs',signed])
signers=re.findall(r'^Signer #\d+ certificate SHA-256 digest: ([0-9a-f]{64})$',commands[-1]['output'],re.M)
if len(signers)!=1: raise ValueError('one verified development signer required')
run([tools/'zipalign','-c','-P','16','-v','4',signed])
members=[]
with zipfile.ZipFile(signed) as z:
  for i in z.infolist(): members.append({'name':i.filename,'bytes':i.file_size,'compressed_bytes':i.compress_size,'sha256':hashlib.sha256(z.read(i)).hexdigest()})
(build/'artifact-manifest.json').write_text(json.dumps({'application_id':args.application_id,'signer_sha256':signers[0],'apk':str(signed),'apk_sha256':hashlib.sha256(signed.read_bytes()).hexdigest(),'members':members},indent=2))
print(json.dumps({'application_id':args.application_id,'signer_sha256':signers[0],'apk':str(signed),'apk_sha256':hashlib.sha256(signed.read_bytes()).hexdigest(),'commands':len(commands)}))
