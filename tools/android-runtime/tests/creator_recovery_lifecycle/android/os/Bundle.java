// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package android.os; import java.util.*; public class Bundle {final Map<String,Object> values=new HashMap<>(); public void putString(String k,String v){values.put(k,v);}public String getString(String k){return (String)values.get(k);}public void putLong(String k,long v){values.put(k,v);}public long getLong(String k){return (Long)values.get(k);}public void putBoolean(String k,boolean v){values.put(k,v);}public boolean getBoolean(String k){return Boolean.TRUE.equals(values.get(k));}public void putByteArray(String k,byte[] v){values.put(k,v);}public byte[] getByteArray(String k){return (byte[])values.get(k);}}
