#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Build the isolated operator harness using SDK tools, no product Gradle tree."""
import argparse, hashlib, json, os, pathlib, re, shutil, subprocess, sys, time, zipfile
import xml.etree.ElementTree as ET
from provenance import source_manifest
from audio_package import audio_package, verify_audio_archive
sys.path.insert(0,str(pathlib.Path(__file__).resolve().parents[2]/'scripts'))
import package_mobile_pdfium
import package_mobile_media
p=argparse.ArgumentParser(); p.add_argument('--root',required=True); p.add_argument('--sdk',required=True); p.add_argument('--java',required=True); p.add_argument('--application-id', default='app.solstone.development.journal'); p.add_argument('--debug-keystore',required=True,type=pathlib.Path); p.add_argument('--owner-app',action='store_true'); p.add_argument('--version-code',type=int,default=1); p.add_argument('--audio-model',type=pathlib.Path); p.add_argument('--asset-catalog',type=pathlib.Path); p.add_argument('--screen-ocr-models',type=pathlib.Path,default=os.environ.get('SCREEN_OCR_MODELS_DIR')); p.add_argument('--detector-model',type=pathlib.Path,default=os.environ.get('RFDETR_MODEL'))
p.add_argument('--sound-tag-model',type=pathlib.Path,default=os.environ.get('CED_MODEL'))
args=p.parse_args()
if args.version_code < 1: p.error('positive version code required')
if args.owner_app and (not args.audio_model or not args.asset_catalog): p.error('owner builds require the pinned speech model and current native asset catalog')
if args.owner_app and not args.screen_ocr_models: p.error('owner builds require the pinned screen OCR model directory')
if not re.fullmatch(r'[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)+', args.application_id): p.error('invalid application id')
key=args.debug_keystore.resolve(strict=True)
if not key.is_file(): p.error('regular retained debug keystore required')
root=pathlib.Path(args.root).resolve(); sdk=pathlib.Path(args.sdk); java=pathlib.Path(args.java); build=root/'build'; build.mkdir(exist_ok=True)
(build/'source-manifest.json').write_text(json.dumps(source_manifest(root.parents[1]),indent=2)+'\n')
tools=sdk/'build-tools/35.0.0'; jar=sdk/'platforms/android-36/android.jar'; commands=[]
audio_manifest,audio_members=audio_package(root.parents[1],args.audio_model,args.asset_catalog,args.sound_tag_model) if args.owner_app else (None,[])
media_manifest,media_members=package_mobile_media.media_package(root.parents[1],args.detector_model or root.parents[1]/'core/models/assets/rfdetr/rfdetr-nano-f16.gguf',args.screen_ocr_models) if args.owner_app else (None,[])
pdfium=root/'build/pdfium'
pdfium_members=package_mobile_pdfium.android_members(pdfium,root)
def run(argv):
  start=time.time(); out=subprocess.run([str(a) for a in argv],stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,timeout=120)
  commands.append({'argv':[str(a) for a in argv],'exit':out.returncode,'elapsed_seconds':time.time()-start,'output':out.stdout.decode(errors='replace')})
  (build/'build-receipts.json').write_text(json.dumps(commands,indent=2))
  if out.returncode: raise RuntimeError(commands[-1])
run([sys.executable,root/'tests/test_audio_package.py'])
run([sys.executable,root.parents[1]/'scripts/test_mobile_pdfium.py'])
run([sys.executable,root.parents[1]/'scripts/test_mobile_media.py'])
classes=build/'classes'; dex=build/'dex'; host_classes=build/'host-tests'
for generated in [classes,dex,host_classes]:
  if generated.exists():shutil.rmtree(generated)
  generated.mkdir()
owner=root/'src/app/solstone/development/journal'
run([java/'bin/javac','-d',host_classes,owner/'ThinkingAdmission.java',owner/'JournalExecution.java',owner/'ServingConnection.java',owner/'PackagedPayload.java',owner/'PackagedAudio.java',owner/'OwnerOrigin.java',owner/'OwnerAccept.java',owner/'OwnerBytes.java',owner/'OwnerBridgeFiles.java',owner/'OwnerPipePair.java',owner/'OwnerUploadBoard.java',owner/'OwnerFolderPicker.java',owner/'OwnerTransferWait.java',root/'tests/ThinkingAdmissionTest.java',root/'tests/PackagedPayloadTest.java',root/'tests/OwnerOriginTest.java',root/'tests/PackagedAudioTest.java',root/'tests/OwnerAcceptTest.java',root/'tests/OwnerBytesTest.java',root/'tests/OwnerBridgeFilesTest.java',root/'tests/OwnerPipePairTest.java',root/'tests/OwnerUploadBoardTest.java',root/'tests/OwnerFolderPickerTest.java',root/'tests/OwnerTransferWaitTest.java'])
java_tmp=build/'java-tmp'; java_tmp.mkdir(exist_ok=True)
run([java/'bin/java','-Djava.io.tmpdir='+str(java_tmp),'-cp',host_classes,'app.solstone.development.journal.PackagedAudioTest'])
run([java/'bin/java','-Djava.io.tmpdir='+str(java_tmp),'-cp',host_classes,'app.solstone.development.journal.ThinkingAdmissionTest'])
run([java/'bin/java','-Djava.io.tmpdir='+str(java_tmp),'-cp',host_classes,'app.solstone.development.journal.PackagedPayloadTest'])
run([java/'bin/java','-Djava.io.tmpdir='+str(java_tmp),'-cp',host_classes,'app.solstone.development.journal.OwnerOriginTest'])
run([java/'bin/java','-Djava.io.tmpdir='+str(java_tmp),'-cp',host_classes,'app.solstone.development.journal.OwnerAcceptTest'])
run([java/'bin/java','-Djava.io.tmpdir='+str(java_tmp),'-cp',host_classes,'app.solstone.development.journal.OwnerBytesTest'])
run([java/'bin/java','-Djava.io.tmpdir='+str(java_tmp),'-cp',host_classes,'app.solstone.development.journal.OwnerBridgeFilesTest'])
run([java/'bin/java','-Djava.io.tmpdir='+str(java_tmp),'-cp',host_classes,'app.solstone.development.journal.OwnerPipePairTest'])
run([java/'bin/java','-Djava.io.tmpdir='+str(java_tmp),'-cp',host_classes,'app.solstone.development.journal.OwnerUploadBoardTest'])
run([java/'bin/java','-Djava.io.tmpdir='+str(java_tmp),'-cp',host_classes,'app.solstone.development.journal.OwnerFolderPickerTest'])
run([java/'bin/java','-Djava.io.tmpdir='+str(java_tmp),'-cp',host_classes,'app.solstone.development.journal.OwnerTransferWaitTest'])
run([java/'bin/javac','-source','8','-target','8','-cp',jar,'-d',classes,*sorted((root/'src').rglob('*.java')),root/'folder-copy/OwnerFolderCopy.java',root/'folder-copy/OwnerTreeSource.java'])
env=os.environ.copy(); env['JAVA_HOME']=str(java); env['PATH']=str(java/'bin')+':'+env.get('PATH',''); os.environ.update(env)
run([tools/'d8','--lib',jar,'--min-api','31','--output',dex,*sorted(classes.rglob('*.class'))])
manifest=ET.parse(root/'AndroidManifest.xml'); original=manifest.getroot().attrib['package']
manifest.getroot().set('package',args.application_id)
for element in manifest.iter():
  name=element.get('{http://schemas.android.com/apk/res/android}name')
  if name and name.startswith('.'): element.set('{http://schemas.android.com/apk/res/android}name',original+name)
manifest_path=build/'AndroidManifest.xml'; manifest.write(manifest_path,encoding='utf-8',xml_declaration=True)
if args.owner_app:
  android='{http://schemas.android.com/apk/res/android}'
  application=manifest.getroot().find('application'); application.set(android+'label','journal')
  application.set(android+'icon','@mipmap/ic_journal')
  application.set(android+'theme','@android:style/Theme.DeviceDefault.DayNight')
  for activity in application.findall('activity'):
    activity.set(android+'exported','false')
    for intent_filter in activity.findall('intent-filter'): activity.remove(intent_filter)
  activity=ET.SubElement(application,'activity',{android+'name':original+'.OwnerActivity',android+'exported':'true',android+'launchMode':'singleTask'})
  intent_filter=ET.SubElement(activity,'intent-filter')
  ET.SubElement(intent_filter,'action',{android+'name':'android.intent.action.MAIN'})
  ET.SubElement(intent_filter,'category',{android+'name':'android.intent.category.LAUNCHER'})
  manifest.write(manifest_path,encoding='utf-8',xml_declaration=True)
unsigned=build/'unsigned.apk'
resources=[]
if args.owner_app:
  compiled=build/'resources.zip'
  run([tools/'aapt2','compile','--dir',root/'res','-o',compiled])
  resources=['-R',compiled]
run([tools/'aapt2','link','-I',jar,'--manifest',manifest_path,'--min-sdk-version','31','--target-sdk-version','36','--version-code',str(args.version_code),'--version-name','internal-'+str(args.version_code),*resources,'-o',unsigned])
with zipfile.ZipFile(unsigned,'a',compression=zipfile.ZIP_DEFLATED) as z:
  z.write(dex/'classes.dex','classes.dex')
  z.write(root.parents[1]/'contracts/journal-web-host/v1.json','assets/journal-web-host.json')
  if audio_manifest:
    z.writestr('assets/audio-models/manifest.json',json.dumps(audio_manifest,sort_keys=True))
    for source,name in audio_members: z.write(source,name)
  if media_manifest:
    z.writestr('assets/journal-payload/media-models.json',json.dumps(media_manifest,sort_keys=True))
    for source,name in media_members: z.write(source,'assets/journal-payload/'+name)
  for native in sorted((root/'lib').rglob('*.so')): z.write(native,str(native.relative_to(root)))
  for source,name in pdfium_members: z.write(source,name)
  for asset in sorted((root/'assets').rglob('*')):
    if asset.is_file(): z.write(asset,str(asset.relative_to(root)))
  payload=root.parents[1]/'core/payload'
  for asset in sorted((payload/'solstone').rglob('*')):
    if asset.is_file(): z.write(asset,'assets/journal-payload/'+str(asset.relative_to(payload)))
with zipfile.ZipFile(unsigned) as z:
  if audio_manifest: verify_audio_archive(z,audio_manifest)
  package_mobile_pdfium.verify_android_archive(pdfium,z)
  if media_manifest: package_mobile_media.verify_archive(z,media_manifest,'assets/journal-payload/')
aligned=build/'aligned.apk'; run([tools/'zipalign','-P','16','-f','4',unsigned,aligned])
signed=build/'journal-process-runtime.apk'
run([tools/'apksigner','sign','--ks',key,'--ks-pass','pass:android','--key-pass','pass:android','--ks-key-alias','androiddebugkey','--out',signed,aligned])
run([tools/'apksigner','verify','--verbose','--print-certs',signed])
signers=re.findall(r'^Signer #\d+ certificate SHA-256 digest: ([0-9a-f]{64})$',commands[-1]['output'],re.M)
if len(signers)!=1: raise ValueError('one verified development signer required')
run([tools/'zipalign','-c','-P','16','-v','4',signed])
members=[]
with zipfile.ZipFile(signed) as z:
  if audio_manifest: verify_audio_archive(z,audio_manifest)
  package_mobile_pdfium.verify_android_archive(pdfium,z)
  if media_manifest: package_mobile_media.verify_archive(z,media_manifest,'assets/journal-payload/')
  for i in z.infolist():
    with z.open(i) as source: member_digest=hashlib.file_digest(source,'sha256').hexdigest()
    members.append({'name':i.filename,'bytes':i.file_size,'compressed_bytes':i.compress_size,'sha256':member_digest})
with signed.open('rb') as stream: apk_digest=hashlib.file_digest(stream,'sha256').hexdigest()
(build/'artifact-manifest.json').write_text(json.dumps({'application_id':args.application_id,'owner_app':args.owner_app,'version_code':args.version_code,'signer_sha256':signers[0],'apk':str(signed),'apk_sha256':apk_digest,'pdfium_receipt_sha256':hashlib.sha256((pdfium/'receipt.json').read_bytes()).hexdigest(),'media_models':media_manifest,'members':members},indent=2))
print(json.dumps({'application_id':args.application_id,'signer_sha256':signers[0],'apk':str(signed),'apk_sha256':apk_digest,'commands':len(commands)}))
