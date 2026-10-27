// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package android.os; public class Message {public int arg1,sendingUid;public Messenger replyTo;Bundle data; public static Message obtain(Object unused,int what){return new Message();}public void setData(Bundle b){data=b;}public Bundle getData(){return data;}}
