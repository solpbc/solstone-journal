#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Exercise the real Android download coordinator with controllable platform IO."""
import argparse
from pathlib import Path
import subprocess
import tempfile

STUBS = {
'android/app/Activity.java': 'package android.app; public class Activity {public static final int RESULT_OK=-1;}',
'android/app/AlertDialog.java': '''package android.app;
public class AlertDialog { public boolean dismissed; public void dismiss(){dismissed=true;}
 public interface Click {void run(Object d,int w);} public interface Cancel {void run(Object d);}
 public static class Builder {public Builder(Object a){} public Builder setMessage(String s){return this;}
 public Builder setNegativeButton(int n,Click c){return this;} public Builder setOnCancelListener(Cancel c){return this;}
 public AlertDialog show(){return new AlertDialog();}}}''',
'android/R.java': 'package android; public class R {public static class string {public static final int cancel=0;}}',
'android/net/Uri.java': 'package android.net; public class Uri {}',
'android/provider/DocumentsContract.java': '''package android.provider; import android.net.Uri;
public class DocumentsContract { public static int deletes;
 public static boolean deleteDocument(Object r, Uri u){ deletes++; return true; } }''',
'android/content/Intent.java': '''package android.content; import android.net.Uri;
public class Intent {public static final String ACTION_CREATE_DOCUMENT="create",CATEGORY_OPENABLE="open",EXTRA_TITLE="title";
 public Intent(String a){} public Intent addCategory(String c){return this;} public Intent setType(String t){return this;}
 public Intent putExtra(String n,String v){return this;} public Uri getData(){return new Uri();}}''',
'android/webkit/URLUtil.java': 'package android.webkit; public class URLUtil {public static String guessFileName(String a,String b,String c){return "journal.zip";}}',
'android/util/Base64.java': 'package android.util; public class Base64 {public static final int DEFAULT=0; public static byte[] decode(String s,int f){return java.util.Base64.getDecoder().decode(s);}}',
'android/util/Log.java': 'package android.util; public class Log {public static void w(String a,String b){} public static void w(String a,String b,Throwable t){}}',
'org/json/JSONArray.java': 'package org.json; public class JSONArray {}',
'app/solstone/development/journal/OwnerActivity.java': '''package app.solstone.development.journal;
import java.io.*;import java.util.*;import java.util.concurrent.Executor;import android.content.Intent;import android.net.Uri;
class OwnerActivity {String pageToken="page";boolean ownerStopped,destroyed;OwnerTransport transport=new OwnerTransport();
 static class Queue {ArrayDeque<Runnable> tasks=new ArrayDeque<>();void post(Runnable r){tasks.add(r);} void drain(){while(!tasks.isEmpty())tasks.remove().run();}}
 final Queue ui=new Queue();final Queue jobs=new Queue();final Executor uploads=r->jobs.post(r);final ArrayList<String> notices=new ArrayList<>();
 File cache;OutputStream output;Runnable opening=()->{};int pickerCalls;
 OwnerActivity(File cache){this.cache=cache;}File getCacheDir(){return cache;}
 void startActivityForResult(Intent i,int code){pickerCalls++;}void notice(String s){notices.add(s);}
 OwnerActivity getContentResolver(){return this;}OutputStream openOutputStream(Uri u,String m)throws IOException {opening.run();return output;}}
''',
'app/solstone/development/journal/OwnerTransport.java': '''package app.solstone.development.journal;
import java.io.*;import java.util.*;import org.json.JSONArray;
class OwnerTransport {byte[] bytes=new byte[]{0,1,2,3};String length="4";String sha256=null;int status=200;ArrayList<String> methods=new ArrayList<>();
 class Cancellation {void cancel(){}}class Stream extends ByteArrayInputStream {int status;Map<String,String> headers=new HashMap<>();
 Stream(String method){super("HEAD".equals(method)?new byte[0]:bytes);status=OwnerTransport.this.status;if(length!=null)headers.put("content-length",length);if(sha256!=null)headers.put("x-solstone-export-sha256",sha256);}}
 Stream follow(String m,String u,String b,JSONArray h,Cancellation c){methods.add(m);return new Stream(m);}}
''',
'app/solstone/development/journal/OwnerDownloadsTest.java': '''package app.solstone.development.journal;
import java.io.*;import java.nio.file.*;import java.util.*;import android.content.Intent;
public class OwnerDownloadsTest {
 static void check(boolean ok){if(!ok)throw new AssertionError();}
 static String chunk(byte[] bytes){return Base64.getEncoder().encodeToString(bytes);}
 static String blob(OwnerDownloads d,byte[] bytes){String id=d.beginBlob("page","key.txt","text/plain",bytes.length);check(!id.isEmpty());check(d.blobChunk("page",id,chunk(bytes)));return id;}
 static void result(OwnerDownloads d){d.selected(-1,new Intent("save"));}
 public static void main(String[] args)throws Exception {
  File cache=new File(args[0]);OwnerActivity a=new OwnerActivity(cache);OwnerDownloads d=new OwnerDownloads(a);
  byte[] bytes={0,1,2,-1,13,10};String id=blob(d,bytes);d.endBlob("page",id,true);a.ui.drain();
  ByteArrayOutputStream output=new ByteArrayOutputStream();a.output=output;result(d);a.jobs.drain();check(a.notices.isEmpty());a.ui.drain();
  check(Arrays.equals(bytes,output.toByteArray()));check(a.notices.size()==1);String saved=a.notices.get(0);a.notices.clear();check(d.active==null);
  // An incomplete Blob cannot offer a picker.
  int picks=a.pickerCalls;id=d.beginBlob("page","key","text/plain",4);d.blobChunk("page",id,chunk(new byte[]{1}));d.endBlob("page",id,true);a.ui.drain();check(a.pickerCalls==picks);check(d.active==null);a.notices.clear();
  // A retired chooser retains ownership until its late result returns.
  id=blob(d,bytes);d.endBlob("page",id,true);a.ui.drain();d.retire();
  check(d.beginBlob("page","new","text/plain",1).isEmpty());result(d);a.ui.drain();check(d.active==null);a.notices.clear();
  // Close failure must never publish success.
  id=blob(d,bytes);d.endBlob("page",id,true);a.ui.drain();a.output=new ByteArrayOutputStream(){public void close()throws IOException{throw new IOException("close failed");}};
  result(d);a.jobs.drain();a.ui.drain();check(a.notices.size()==1);check(!a.notices.get(0).equals(saved));a.notices.clear();
  // Retirement during destination opening closes the late stream and writes nothing.
  final boolean[] closed={false};output=new ByteArrayOutputStream(){public void close(){closed[0]=true;}};a.output=output;a.opening=d::retire;
  id=blob(d,bytes);d.endBlob("page",id,true);a.ui.drain();result(d);a.jobs.drain();a.ui.drain();check(output.size()==0 && closed[0]);check(a.notices.isEmpty());a.opening=()->{};
  // HTTP failure, truncation, excess bytes and absent length never offer save.
  for(String length:new String[]{"5","3",null}) {picks=a.pickerCalls;a.transport.length=length;d.receive("https://journal.invalid/export","","application/zip");a.jobs.drain();a.ui.drain();check(a.pickerCalls==picks);check(d.active==null);a.notices.clear();}
  a.transport.length="4";a.transport.status=500;picks=a.pickerCalls;d.receive("https://journal.invalid/export","","application/zip");a.jobs.drain();a.ui.drain();check(a.pickerCalls==picks);a.notices.clear();
  a.transport.status=200;d.receive("https://journal.invalid/export","","application/zip");a.jobs.drain();a.ui.drain();check(a.pickerCalls==picks+1);output=new ByteArrayOutputStream();a.output=output;result(d);a.jobs.drain();a.ui.drain();check(Arrays.equals(output.toByteArray(),a.transport.bytes));
  // Prepared exports pick SAF first with zero cache staging, stream once, and verify SHA-256.
  a.notices.clear();a.transport.methods.clear();
  a.transport.sha256="054edec1d0211f624fed0cbca9d4f9400b0e491c43742af2c5b0abebf0c990d8"; // sha256({0,1,2,3})
  picks=a.pickerCalls;d.receive("https://journal.invalid/app/export/api/ready/exp_1/download","","application/zip");
  a.jobs.drain();a.ui.drain();check(a.pickerCalls==picks+1);check(cache.list().length==0);check(a.transport.methods.equals(List.of("HEAD")));
  output=new ByteArrayOutputStream();a.output=output;result(d);a.jobs.drain();a.ui.drain();
  check(Arrays.equals(output.toByteArray(),a.transport.bytes));check(a.transport.methods.equals(List.of("HEAD","GET")));check(a.notices.size()==1 && a.notices.get(0).contains("copy saved"));
  // Checksum mismatch deletes the partial SAF document and refuses success.
  a.notices.clear();int prevDeletes=android.provider.DocumentsContract.deletes;
  a.transport.sha256="0000000000000000000000000000000000000000000000000000000000000000";
  d.receive("https://journal.invalid/app/export/api/ready/exp_1/download","","application/zip");a.jobs.drain();a.ui.drain();
  output=new ByteArrayOutputStream();a.output=output;result(d);a.jobs.drain();a.ui.drain();
  check(android.provider.DocumentsContract.deletes==prevDeletes+1);check(a.notices.size()==1 && !a.notices.get(0).contains("copy saved"));
  // Direct SAF stream close failure deletes document and never reports copy saved.
  a.notices.clear();prevDeletes=android.provider.DocumentsContract.deletes;
  a.transport.sha256="054edec1d0211f624fed0cbca9d4f9400b0e491c43742af2c5b0abebf0c990d8";
  d.receive("https://journal.invalid/app/export/api/ready/exp_1/download","","application/zip");a.jobs.drain();a.ui.drain();
  a.output=new ByteArrayOutputStream(){public void close()throws IOException{throw new IOException("saf close failed");}};
  result(d);a.jobs.drain();a.ui.drain();
  check(android.provider.DocumentsContract.deletes==prevDeletes+1);check(a.notices.size()==1 && !a.notices.get(0).contains("copy saved"));
  // Picker cancellation (RESULT_CANCELED=0) cleans active state cleanly with no notice.
  a.notices.clear();a.transport.methods.clear();
  d.receive("https://journal.invalid/app/export/api/ready/exp_1/download","","application/zip");a.jobs.drain();a.ui.drain();
  d.selected(0,null);a.jobs.drain();a.ui.drain();
  check(d.active==null);check(a.notices.isEmpty());check(a.transport.methods.equals(List.of("HEAD")));
  // Stale journal-export-*.tmp left by prior process exit is swept on next startup.
  File stale=new File(cache,"journal-export-stale-exit.tmp");
  try(FileOutputStream fos=new FileOutputStream(stale)){fos.write(new byte[]{1,2,3});}
  check(stale.exists());
  new OwnerDownloads(a);
  check(!stale.exists());
  check(cache.list().length==0);System.out.println("native downloads: byte fidelity, incomplete source, picker ownership, close failure, zero-cache SAF export, cancel and process-exit cleanup pass");
 }}'''
}

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--java-home', type=Path, required=True)
    args = parser.parse_args()
    owner = Path(__file__).resolve().parent.parent / 'src/app/solstone/development/journal'
    with tempfile.TemporaryDirectory(prefix='owner-downloads-') as directory:
        root = Path(directory)
        for name, text in STUBS.items():
            target = root / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(text)
        classes = root / 'classes'
        files = root / 'files'
        files.mkdir()
        subprocess.run([str(args.java_home / 'bin/javac'), '-d', str(classes),
                        *map(str, root.rglob('*.java')), str(owner / 'OwnerDownloads.java'),
                        str(owner / 'OwnerOrigin.java')], check=True, timeout=60)
        subprocess.run([str(args.java_home / 'bin/java'), '-cp', str(classes),
                        'app.solstone.development.journal.OwnerDownloadsTest', str(files)], check=True, timeout=30)

if __name__ == '__main__':
    main()
