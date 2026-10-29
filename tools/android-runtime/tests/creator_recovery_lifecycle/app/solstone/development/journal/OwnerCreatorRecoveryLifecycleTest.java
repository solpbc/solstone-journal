// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package app.solstone.development.journal;
import android.os.*;import static app.solstone.development.journal.CreatorRecoveryState.*;import java.nio.file.*;import java.util.*;import java.util.concurrent.*;import java.util.concurrent.atomic.*;
public class OwnerCreatorRecoveryLifecycleTest {
static void require(boolean value,String why){if(!value)throw new AssertionError(why);}
static class Service {
 final OwnerActivity owner;final ExecutorService work=Executors.newSingleThreadExecutor();final AtomicLong epoch=new AtomicLong(1);final CountDownLatch observed=new CountDownLatch(1),stopEntered=new CountDownLatch(1),stopRelease=new CountDownLatch(1);volatile boolean dropObserve,blockStop;volatile Messenger incoming;final Dispatcher dispatcher;
 Service(OwnerActivity owner){this.owner=owner;dispatcher=new Dispatcher(11285,owner.root,new FilesStore(owner.files,"creator-recovery-service"),new Host(){public long epoch(){return Service.this.epoch.get();}public void cancel(){Service.this.epoch.addAndGet(2);}public void execute(Runnable r){work.execute(r);}public Result run(Request r)throws Exception {if(r.action==Action.STOP){if(blockStop){stopEntered.countDown();require(stopRelease.await(10,TimeUnit.SECONDS),"Stop control never released");}return new Result("actual joined stop",r.id,null,epoch(),null,null,true,false,false,true);}if(r.action==Action.OBSERVE){observed.countDown();return new Result("actual readonly return",r.id,"original-root",epoch(),"empty population","revision",true,false,false,false);}throw new AssertionError("unexpected native action "+r.action);}});}
 Messenger bind(){incoming=new Messenger(new Handler(Looper.getMainLooper()){public void handleMessage(Message m){try{Request request=Request.decode(m.getData().getByteArray("request"));dispatcher.receive(m.sendingUid,m.replyTo.getBinder(),request,new Receiver(){public void returned(Request subject,Result value,boolean saved){if(dropObserve&&request.action==Action.OBSERVE)return;send(subject,value,saved);}public void pending(Request subject){send(subject,null,false);}public void refused(Request subject){send(subject,Result.refused(subject),true);}void send(Request subject,Result value,boolean saved){try{Message answer=Message.obtain(null,1);answer.arg1=m.arg1;answer.replyTo=incoming;Bundle b=new Bundle();b.putString("query",request.id);b.putString("nonce",request.nonce);b.putLong("connection",request.connection);b.putString("root",request.root);b.putByteArray("subject",subject.encoded());if(value!=null)b.putByteArray("result",value.encoded());b.putBoolean("persisted",saved);answer.setData(b);m.replyTo.send(answer);}catch(Exception error){throw new RuntimeException(error);}}});}catch(Exception error){throw new RuntimeException(error);}}});return incoming;}
}
static void control(boolean olderApply)throws Exception {
 Path temp=Files.createTempDirectory("actual-owner-rebind-");OwnerActivity owner=new OwnerActivity(temp.toFile());Service service=new Service(owner);owner.worker=service.bind();OwnerCreatorRecovery recovery=new OwnerCreatorRecovery(owner);OwnerCreatorRecovery.Session old=recovery.current();
 try {
  old.stop();recovery.busy=true;service.dropObserve=true;
  Future<?> lost=owner.requests.submit(()->{try{old.call(old.request(Action.OBSERVE,null,null),recovery.choice.get());throw new AssertionError("lost reply became completion");}catch(Exception expected){}});
  require(service.observed.await(10,TimeUnit.SECONDS),"actual old observation never dispatched");
  FilesStore clientStore=recovery.store();Request prior=null;
  if(olderApply){prior=new Request("older-possible-apply","old-nonce",1,owner.root,Action.APPLY,"old exact body","original-root","old-review",3,"old full confirmation","old revision",null,null);clientStore.request(prior,Phase.SENT);}
  recovery.changed();lost.get(10,TimeUnit.SECONDS);owner.connectionGeneration.incrementAndGet();owner.worker=service.bind();
  service.blockStop=true;service.dropObserve=false;recovery.stopping();
  require(service.stopEntered.await(1,TimeUnit.SECONDS),"new stopped channel was not acquired after connection loss");
  require(recovery.busy,"UI reopened before actual new-channel Stop returned");
  service.stopRelease.countDown();long deadline=System.nanoTime()+TimeUnit.SECONDS.toNanos(10);
  while(recovery.busy&&System.nanoTime()<deadline)Thread.sleep(1);
  require(!recovery.busy,"joined new-channel Stop did not reopen explicit controls");
  require(recovery.startAllowed()!=olderApply,"readonly loss or older apply hold polarity lost");
  if(prior!=null)require(clientStore.get(prior.id).result==null,"Stop rewrote older unknown apply");
  OwnerCreatorRecovery.Session fresh=recovery.current();OwnerCreatorRecovery.Response explicit=fresh.call(fresh.request(Action.OBSERVE,null,null),recovery.choice.get());
  require(explicit.result!=null&&explicit.saved&&explicit.result.confirmation!=null,"fresh explicit review inaccessible after rebind/Stop");
  require(owner.starts==0,"recovery automatically started owner");
 }finally{service.stopRelease.countDown();recovery.changed();owner.requests.shutdownNow();service.work.shutdown();require(service.work.awaitTermination(10,TimeUnit.SECONDS),"service worker did not join");require(owner.requests.awaitTermination(10,TimeUnit.SECONDS),"owner callbacks did not join");try(java.util.stream.Stream<Path> paths=Files.walk(temp)){for(Path p:paths.sorted(Comparator.reverseOrder()).toArray(Path[]::new))Files.delete(p);}}
}
public static void main(String[] args)throws Exception {control(false);control(true);System.out.println("actual OwnerCreatorRecovery lifecycle: readonly lost reply/rebind/joined Stop/fresh explicit review0; prior apply hold twin0; no Start/native/device");}
}
