// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package android.app; public class AlertDialog {public interface Listener{void clicked(Object dialog,int button);} public static class Builder{public Builder(Object c){}public Builder setTitle(String s){return this;}public Builder setMessage(String s){return this;}public Builder setNegativeButton(String s,Listener l){return this;}public Builder setPositiveButton(String s,Listener l){return this;}public void show(){throw new AssertionError("dialog outside lifecycle control");}}}
