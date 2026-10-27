// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package app.solstone.development.journal;
import android.os.*;import java.io.*;import java.util.concurrent.*;import java.util.concurrent.atomic.*;
final class OwnerActivity {final File files;final String root;volatile boolean destroyed,ownerStopped=true,stopChoiceUnsaved;volatile String runState="stopped",reason="";volatile Messenger worker;final AtomicLong connectionGeneration=new AtomicLong(1);final AtomicInteger sequence=new AtomicInteger();final ExecutorService requests=Executors.newFixedThreadPool(2);final Handler ui=new Handler(Looper.getMainLooper());int starts;OwnerActivity(File files){this.files=files;root=new File(files,"synthetic-serving-journal").toString();}File getFilesDir(){return files;}boolean restoreOwnerStop(){return true;}boolean currentConnection(long generation,Messenger target){return !destroyed&&connectionGeneration.get()==generation&&worker==target;}void renderBar(){} }
