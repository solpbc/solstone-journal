// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import java.util.HashSet;
import java.util.Set;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;

public final class SourcePermissionTransitionTest {
    static final String URI="content://synthetic/tree/notes";
    static final class Access implements SourcePermissionTransition.Access {
        final Set<String> grants=new HashSet<>();
        int takes,releases;
        boolean failTakeAfterEffect,failRelease;
        Runnable afterTake=() -> {};
        public boolean hasRead(String material) { return grants.contains(material); }
        public void takeRead(String material) throws Exception {
            takes++;grants.add(material);afterTake.run();
            if (failTakeAfterEffect) throw new Exception("take reply lost after effect");
        }
        public void releaseRead(String material) throws Exception {
            releases++;
            if (failRelease) throw new Exception("release unavailable");
            grants.remove(material);
        }
    }
    static void check(boolean condition,String message) {
        if (!condition) throw new AssertionError(message);
    }
    public static void main(String[] args) throws Exception {
        Access prior=new Access();prior.grants.add(URI);
        SourcePermissionTransition existing=new SourcePermissionTransition(URI,true,prior,() -> true);
        check(existing.transition(0,URI)==0,"existing read grant");
        check(existing.transition(1,URI)==0 && prior.hasRead(URI) && prior.releases==0,"rollback must preserve an existing grant");

        Access denied=new Access();
        SourcePermissionTransition ephemeral=new SourcePermissionTransition(URI,false,denied,() -> true);
        check(ephemeral.transition(0,URI)==-1 && denied.takes==0,"a nonpersistable picker result cannot become remembered access");

        Access stopped=new Access();AtomicBoolean live=new AtomicBoolean(true);
        stopped.afterTake=() -> live.set(false);
        SourcePermissionTransition interrupted=new SourcePermissionTransition(URI,true,stopped,live::get);
        check(interrupted.transition(0,URI)==-1 && stopped.hasRead(URI),"Stop after acquisition must be observed");
        check(interrupted.transition(1,URI)==0 && !stopped.hasRead(URI),"actual rollback must still retire after Stop");

        Access lostReply=new Access();lostReply.failTakeAfterEffect=true;
        SourcePermissionTransition uncertain=new SourcePermissionTransition(URI,true,lostReply,() -> true);
        check(uncertain.transition(0,URI)==-1 && lostReply.hasRead(URI),"lost reply can leave a real acquired grant");
        check(uncertain.transition(1,URI)==0 && !lostReply.hasRead(URI),"lost acquisition reply must not lose cleanup ownership");

        Access retained=new Access();retained.failRelease=true;
        SourcePermissionTransition retry=new SourcePermissionTransition(URI,true,retained,() -> true);
        check(retry.transition(0,URI)==1,"new remembered grant");
        check(retry.transition(1,URI)==-1 && retained.hasRead(URI),"failed retirement must not acknowledge release");
        retained.failRelease=false;
        check(retry.transition(1,URI)==0 && !retained.hasRead(URI),"same ownership can finish actual retirement");

        Access wrong=new Access();SourcePermissionTransition threadBound=new SourcePermissionTransition(URI,true,wrong,() -> true);
        AtomicInteger otherResult=new AtomicInteger(123);
        Thread other=new Thread(() -> otherResult.set(threadBound.transition(0,URI)));
        other.start();other.join(5000);
        check(!other.isAlive() && otherResult.get()==-1 && wrong.takes==0,"permission work stays on its retained native caller");
        System.out.println("source permission ownership controls passed");
    }
}
