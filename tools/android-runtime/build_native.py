#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Build the selected Journal library and JNI shim on the Android build host."""
import argparse,hashlib,json,os,pathlib,shutil,subprocess,sys,time
from provenance import source_manifest
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--clippy',action='store_true')
parser.add_argument('--release',action='store_true')
args=parser.parse_args()
root=pathlib.Path(__file__).resolve().parent
repo=root.parents[1]
ndk=pathlib.Path(os.environ.get('JOURNAL_NDK','/var/tmp/android-runtime-toolchain/android-ndk-r30'))
bins=ndk/'toolchains/llvm/prebuilt/linux-x86_64/bin'
env=os.environ.copy()
env.update(CARGO_TARGET_DIR=str(root/'target'),CARGO_BUILD_JOBS='2',CARGO_INCREMENTAL='0',CARGO_PROFILE_DEV_DEBUG='0',CC_aarch64_linux_android=str(bins/'aarch64-linux-android31-clang'),AR_aarch64_linux_android=str(bins/'llvm-ar'),CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=str(bins/'aarch64-linux-android31-clang'),CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS='-C link-arg=-Wl,-z,max-page-size=16384 -C link-arg=-Wl,-z,common-page-size=16384')
env.update(CXX_aarch64_linux_android=str(bins/'aarch64-linux-android31-clang++'),CARGO_NDK_SYSROOT_PATH=str(bins.parent/'sysroot'))
cxx_inputs=[bins.parent/'sysroot/usr/lib/aarch64-linux-android'/name for name in ('libc++_static.a','libc++abi.a')]
cxx_dir=root/'build/cxx-runtime';cxx_dir.mkdir(parents=True,exist_ok=True)
for library in cxx_inputs:shutil.copy2(library,cxx_dir/library.name)
# Expose only C++ archives; the NDK's generic directory also has a Rust-based libc.a.
env['CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS'] += ' -L native=' + str(cxx_dir)
for key, value in {'CC':str(bins/'clang'),'CXX':str(bins/'clang++'),'AR':str(bins/'llvm-ar'),'CFLAGS':'--target=aarch64-linux-android31','CXXFLAGS':'--target=aarch64-linux-android31'}.items(): env[key+'_aarch64-linux-android']=value
for key in ('PARAKEET_LIB_DIR','SOLSTONE_FFMPEG_SOURCE_ARCHIVE','JOURNAL_ONNX_LIBRARY','RFDETR_LIB_DIR'):
 if not env.get(key): raise RuntimeError(key+' is required for the native development app')
out=root/'evidence';out.mkdir(exist_ok=True)
(out/'source-manifest.json').write_text(json.dumps(source_manifest(repo),indent=2)+'\n')
steps=[]
def run(name,argv):
 start=time.time();log=out/(name+'.log')
 with log.open('wb') as f:p=subprocess.run(list(map(str,argv)),cwd=repo,env=env,stdin=subprocess.DEVNULL,stdout=f,stderr=subprocess.STDOUT,timeout=1800)
 row={'name':name,'argv':list(map(str,argv)),'exit':p.returncode,'seconds':time.time()-start,'log_sha256':hashlib.sha256(log.read_bytes()).hexdigest()}
 steps.append(row);(out/'native-build.json').write_text(json.dumps({'source':subprocess.check_output(['git','rev-parse','HEAD'],cwd=repo,text=True).strip(),'environment':{k:v for k,v in env.items() if k.startswith(('CARGO_','CC_','CXX_','AR_','CFLAGS_','CXXFLAGS_','PARAKEET_','SOLSTONE_FFMPEG_','JOURNAL_ONNX_','RFDETR_'))},'steps':steps},indent=2))
 print(json.dumps(row),flush=True)
 if p.returncode:print(log.read_text(errors='replace')[-12000:]);sys.exit(p.returncode)
profile_args=['--release'] if args.release else []
run('cargo-build',['cargo','build','--locked','--offline','--manifest-path',root/'native/Cargo.toml','--target','aarch64-linux-android',*profile_args])
lib=root/'lib/arm64-v8a';lib.mkdir(parents=True,exist_ok=True)
onnx=pathlib.Path(env['JOURNAL_ONNX_LIBRARY'])
shutil.copy2(onnx,lib/'libonnxruntime.so')
rfdetr=pathlib.Path(env['RFDETR_LIB_DIR'])
header=pathlib.Path(env['RFDETR_CAPI_HEADER']) if env.get('RFDETR_CAPI_HEADER') else next(iter(sorted(p for p in rfdetr.rglob('rfdetr_capi.h') if p.is_file())), None)
if header is None: raise RuntimeError('rfdetr_capi.h was not found under RFDETR_LIB_DIR')
model=repo/'core/models/assets/rfdetr/rfdetr-nano-f16.gguf'
rfdetr_archives=sorted(p for p in rfdetr.rglob('librfdetr.a') if p.is_file())
if len(rfdetr_archives)!=1: raise RuntimeError('RFDETR_LIB_DIR must contain exactly one librfdetr.a')
inputs=[onnx,*cxx_inputs,pathlib.Path(env['SOLSTONE_FFMPEG_SOURCE_ARCHIVE']),*sorted(pathlib.Path(env['PARAKEET_LIB_DIR']).rglob('*.a')),*rfdetr_archives,header,model]
(out/'audio-native-inputs.json').write_text(json.dumps({str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in inputs},indent=2)+'\n')
profile='release' if args.release else 'debug'
shutil.copy2(root/'target/aarch64-linux-android'/profile/'libandroid_journal_runtime.so',lib/'libandroid_journal_runtime.so')
run('jni-build',[bins/'aarch64-linux-android31-clang','-shared','-fPIC','-Wl,-z,max-page-size=16384','-Wl,-z,common-page-size=16384',root/'bridge.c','-L'+str(lib),'-landroid_journal_runtime','-o',lib/'libjournalbridge.so'])

if args.clippy:run('cargo-clippy',['cargo','clippy','--locked','--offline','--manifest-path',root/'native/Cargo.toml','--target','aarch64-linux-android','--all-targets',*profile_args,'--','-D','warnings'])
