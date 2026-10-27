// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package android.os; public class Messenger {final Handler handler;final IBinder binder=new IBinder(){};public Messenger(Handler h){handler=h;}public IBinder getBinder(){return binder;}public void send(Message m){m.sendingUid=Process.myUid();handler.handleMessage(m);}}
