// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;
import static app.solstone.development.journal.CreatorRecoveryState.*;
import java.io.*;
import java.nio.file.*;
import java.util.*;
import java.util.concurrent.*;
import java.util.concurrent.atomic.*;

public final class CreatorRecoveryStateTest {
    static void require(boolean value,String message){if(!value)throw new AssertionError(message);}
    static final String ROOT="/private/own/journal",NONCE="current-nonce",CONFIRM="complete-original-and-stage-snapshot",REVISION="snapshot-revision";
    static final int UID=11285;
    static final Object BINDER=new Object(),WORKER=new Object();
    static Request request(String id,Action action,long epoch){return request(id,action,epoch,null,null);}
    static Request request(String id,Action action,long epoch,String prior,String digest) {
        return new Request(id,NONCE,1,ROOT,action,action+":"+id+":"+epoch,
            action==Action.STOP?null:"original-root",action==Action.APPLY||action==Action.RECONCILE?"review":null,epoch,
            action==Action.APPLY||action==Action.RECONCILE?CONFIRM:null,
            action==Action.APPLY||action==Action.RECONCILE?REVISION:null,prior,digest);
    }
    static final class Replies implements Receiver {
        int refusals,pending,returns;Request subject;Result result;boolean saved;
        public void returned(Request r,Result value,boolean persisted){subject=r;result=value;saved=persisted;returns++;}
        public void pending(Request r){subject=r;pending++;}
        public void refused(Request r){subject=r;refusals++;}
    }
    static final class Engine implements Host {
        final AtomicLong epoch=new AtomicLong(1);final ArrayDeque<Runnable> queue=new ArrayDeque<>();
        int calls,applies,stops;Runnable onApply;boolean partial;long observationEpoch=-1;
        public long epoch(){return epoch.get();}
        public void cancel(){epoch.addAndGet(2);}
        public void execute(Runnable work){queue.add(work);}
        public Result run(Request r) {
            calls++;
            if(r.action==Action.STOP){stops++;return new Result("actual-stop",r.id,null,epoch(),null,null,true,false,false,true);}
            if(r.action==Action.OBSERVE){observationEpoch=epoch();return new Result("actual-review",r.id,"original-root",epoch(),CONFIRM,REVISION,true,false,false,false);}
            if(r.action==Action.APPLY){applies++;if(onApply!=null)onApply.run();}
            return new Result("actual-"+r.action,r.id,"original-root",epoch(),null,null,false,r.action==Action.APPLY&&!partial,r.action==Action.RECONCILE,false);
        }
        void pump(){while(!queue.isEmpty())queue.remove().run();}
    }
    static final class FailStore implements Store {
        final Store inner;boolean failRequest,failResult;Runnable afterRequest;
        FailStore(Store inner){this.inner=inner;}
        public CreatorRecoveryState.Record get(String id)throws Exception{return inner.get(id);}
        public List<CreatorRecoveryState.Record> all()throws Exception{return inner.all();}
        public void request(Request r,Phase phase)throws Exception{if(failRequest)throw new IOException("controlled pre-send refusal");inner.request(r,phase);if(afterRequest!=null)afterRequest.run();}
        public void result(Request r,Result value)throws Exception{if(failResult)throw new IOException("controlled result-save refusal");inner.result(r,value);}
    }
    static final class Fixture implements AutoCloseable {
        final Path directory;final FilesStore store;final Engine engine=new Engine();final Dispatcher dispatcher;
        Fixture()throws Exception{directory=Files.createTempDirectory("creator-receiving-");store=new FilesStore(directory.toFile(),"service");dispatcher=new Dispatcher(UID,ROOT,store,engine);}
        void stoppedAndReviewed()throws Exception {
            Replies stopped=new Replies();dispatcher.receive(UID,BINDER,request("joined-stop",Action.STOP,1),stopped);engine.pump();
            require(stopped.result!=null&&stopped.result.stopped,"real Stop result missing");
            Replies reviewed=new Replies();dispatcher.receive(UID,BINDER,request("review",Action.OBSERVE,engine.epoch()),reviewed);engine.pump();
            require(reviewed.result!=null&&CONFIRM.equals(reviewed.result.confirmation),"real review missing");
        }
        public void close()throws Exception{try(java.util.stream.Stream<Path> files=Files.walk(directory)){Path[] paths=files.sorted(Comparator.reverseOrder()).toArray(Path[]::new);for(Path p:paths)Files.delete(p);}}
    }
    static void fixedAuthorityDuplicateAndStaleControls()throws Exception {
        try(Fixture f=new Fixture()){
            require(!genericAllowed("creator-recovery")&&genericAllowed("status"),"generic creator bypass");
            Replies wrong=new Replies();f.dispatcher.receive(UID+1,BINDER,request("wrong-uid",Action.STOP,1),wrong);
            require(wrong.refusals==0&&wrong.returns==0&&f.engine.calls==0,"wrong UID ran native");
            Request wrongRoot=new Request("wrong-root",NONCE,1,"/foreign",Action.STOP,"stop",null,null,1,null,null,null,null);
            f.dispatcher.receive(UID,BINDER,wrongRoot,wrong);require(f.engine.calls==0,"foreign root ran native");
            f.stoppedAndReviewed();int before=f.engine.calls;
            Request r=request("apply",Action.APPLY,f.engine.epoch());
            Replies forged=new Replies();f.dispatcher.receive(UID,new Object(),r,forged);require(forged.refusals==1,"foreign endpoint accepted");
            Request stale=new Request("stale",NONCE,1,ROOT,Action.APPLY,"stale","original-root","invented-review",f.engine.epoch(),CONFIRM,REVISION,null,null);
            f.dispatcher.receive(UID,BINDER,stale,forged);require(f.engine.calls==before,"invented review dispatched");
            Replies actual=new Replies();f.dispatcher.receive(UID,BINDER,r,actual);f.engine.pump();
            require(actual.result.complete&&actual.saved&&f.engine.applies==1,"apply did not run once");
            // Exact duplicate may recover the original bytes, never call Native twice.
            f.dispatcher.receive(UID,BINDER,r,actual);f.engine.pump();require(f.engine.applies==1&&actual.returns==2&&actual.result.complete,"duplicate did not recover original actual result");
            Request changed=new Request(r.id,r.nonce,r.connection,r.root,r.action,r.body+"changed",r.rootObservation,r.observationId,r.epoch,r.confirmation,r.revision,null,null);
            f.dispatcher.receive(UID,BINDER,changed,forged);f.engine.pump();require(f.engine.applies==1&&forged.refusals>=1,"changed body reused id");
        }
    }
    static void queuedCloseAndEpochRefuseBeforeWork()throws Exception {
        for(boolean close:new boolean[]{false,true})try(Fixture f=new Fixture()){
            f.stoppedAndReviewed();Request r=request("queued-apply",Action.APPLY,f.engine.epoch());Replies response=new Replies();
            f.dispatcher.receive(UID,BINDER,r,response);require(held(f.store,ROOT),"possible admission not held before work");
            if(close)f.dispatcher.close(BINDER);else f.engine.cancel();f.engine.pump();
            require(f.engine.applies==0&&response.result.noEffect,"stale queued request ran native");
            require(!held(f.store,ROOT),"actual no-dispatch result did not settle local receiving");
        }
    }
    static void clientPersistenceAndReplyAuthentication()throws Exception {
        try(Fixture f=new Fixture()){
            FailStore store=new FailStore(new FilesStore(f.directory.toFile(),"client"));AtomicBoolean current=new AtomicBoolean(true);AtomicInteger sends=new AtomicInteger();
            Client client=new Client(UID,WORKER,ROOT,NONCE,1,store,current::get,r->sends.incrementAndGet());
            Request r=request("before-send",Action.APPLY,3);store.failRequest=true;
            try{client.send(r);throw new AssertionError("pre-write failure accepted");}catch(IOException expected){}
            require(sends.get()==0&&!held(store,ROOT),"pre-write failure dispatched");store.failRequest=false;
            store.afterRequest=()->current.set(false);
            try{client.send(request("persistence-race",Action.APPLY,3));throw new AssertionError("persistence race sent");}catch(IOException expected){}
            require(sends.get()==0&&!held(store,ROOT),"changed caller during persistence dispatched");
            store.afterRequest=null;current.set(true);r=request("sent-no-ack",Action.APPLY,3);client.send(r);
            require(sends.get()==1&&held(store,ROOT),"lost admission ACK erased hold");
            Result result=new Result("actual-complete",r.id,"original-root",3,null,null,false,true,false,false);
            require(!client.accept(UID+1,WORKER,r.id,NONCE,1,ROOT,r,result,true),"foreign UID reply accepted");
            require(!client.accept(UID,new Object(),r.id,NONCE,1,ROOT,r,result,true),"foreign worker reply accepted");
            require(!client.accept(UID,WORKER,r.id,NONCE,2,ROOT,r,result,true),"stale connection accepted");
            require(!client.accept(UID,WORKER,"foreign-request",NONCE,1,ROOT,r,result,true),"foreign query accepted");
            store.failResult=true;
            try{client.accept(UID,WORKER,r.id,NONCE,1,ROOT,r,result,true);throw new AssertionError("save failure hidden");}catch(IOException expected){}
            require(held(store,ROOT)&&client.received.get(r.id).complete,"result-save failure lost actual facts or hold");
            store.failResult=false;require(client.accept(UID,WORKER,r.id,NONCE,1,ROOT,r,result,true),"bound result refused");
            require(!held(store,ROOT),"saved actual complete result retained hold");
        }
    }
    static void readonlyPolarityAndUnknownHistory()throws Exception {
        try(Fixture f=new Fixture()){
            FilesStore store=new FilesStore(f.directory.toFile(),"client");
            Client client=new Client(UID,WORKER,ROOT,NONCE,1,store,()->true,r->{});
            client.send(request("lost-first-observe",Action.OBSERVE,3));client.send(request("lost-status",Action.STATUS,3));
            require(!held(store,ROOT),"read-only loss invented apply hold");
            Request old=request("old-unknown",Action.APPLY,3);client.send(old);require(held(store,ROOT),"unknown apply not held");
            client.send(request("later-empty-review",Action.OBSERVE,5));client.send(request("later-stop",Action.STOP,5));
            require(held(store,ROOT)&&store.get(old.id).result==null,"later view or Stop rewrote history");
            Request draft=request("new-reconciliation",Action.RECONCILE,3,old.id,digest(old.body));
            Request reconcile=new Request(draft.id,draft.nonce,draft.connection,draft.root,draft.action,draft.body,draft.rootObservation,
                draft.observationId,draft.epoch,draft.confirmation,draft.revision,draft.priorId,draft.priorDigest,old.body);
            Request foreign=new Request("wrong-root-reconcile",NONCE,1,"/different-root",Action.RECONCILE,"read-only",
                draft.rootObservation,draft.observationId,draft.epoch,draft.confirmation,draft.revision,old.id,digest(old.body),old.body);
            store.request(foreign,Phase.SENT);store.result(foreign,new Result("foreign-result",foreign.id,"original-root",5,null,null,true,false,true,false));
            require(held(store,ROOT),"other-root reconciliation cleared current hold");
            client.send(reconcile);Result current=new Result("current-local-reconciled",reconcile.id,"original-root",5,null,null,true,false,true,false);
            require(client.accept(UID,WORKER,reconcile.id,NONCE,1,ROOT,reconcile,current,true),"bound reconciliation refused");
            require(!held(store,ROOT)&&store.get(old.id).result==null,"reconciliation claimed old success or failed to settle local hold");
        }
    }
    static void exactResultLookupWithoutNativeReplay()throws Exception {
        try(Fixture f=new Fixture()){
            FailStore store=new FailStore(f.store);Dispatcher d=new Dispatcher(UID,ROOT,store,f.engine);
            Replies stop=new Replies();d.receive(UID,BINDER,request("stop-two",Action.STOP,f.engine.epoch()),stop);f.engine.pump();
            Replies review=new Replies();d.receive(UID,BINDER,request("review",Action.OBSERVE,f.engine.epoch()),review);f.engine.pump();
            require(review.result!=null,"review result unavailable");
            store.failResult=true;Request apply=request("lost-service-save",Action.APPLY,f.engine.epoch());Replies actual=new Replies();
            d.receive(UID,BINDER,apply,actual);f.engine.pump();
            require(actual.result.complete&&!actual.saved&&held(store,ROOT),"effectful save failure erased hold");
            int calls=f.engine.calls;store.failResult=false;
            Replies recovered=new Replies();Request lookup=request("lookup-exact",Action.STATUS,f.engine.epoch(),apply.id,digest(apply.body));
            d.receive(UID,BINDER,lookup,recovered);
            require(f.engine.calls==calls&&recovered.result.complete&&recovered.saved&&!held(store,ROOT),"lookup replayed or lost actual result");
        }
    }
    static void duplicatePendingPartialAndLostReplyNeverInventNoEffect()throws Exception {
        for(boolean partial:new boolean[]{false,true})try(Fixture f=new Fixture()){
            f.stoppedAndReviewed();f.engine.partial=partial;
            FilesStore clientStore=new FilesStore(f.directory.toFile(),"client");
            Request apply=request("possible-apply",Action.APPLY,f.engine.epoch());
            AtomicBoolean drop=new AtomicBoolean(true);
            Client client=new Client(UID,WORKER,ROOT,NONCE,1,clientStore,()->true,r->{});
            client.send(apply);require(held(clientStore,ROOT),"sent apply has no hold");
            Replies original=new Replies();f.dispatcher.receive(UID,BINDER,apply,original);
            Receiver receiving=new Receiver(){
                public void returned(Request subject,Result result,boolean saved){
                    require(!result.noEffect,"accepted apply duplicate invented no-effect");
                    if(!drop.get())try{require(client.accept(UID,WORKER,apply.id,NONCE,1,ROOT,subject,result,saved),"original result binding lost");}catch(Exception error){throw new RuntimeException(error);}
                }
                public void pending(Request subject){try{require(held(clientStore,ROOT),"pending duplicate cleared client hold");}catch(Exception error){throw new RuntimeException(error);}}
                public void refused(Request subject){throw new AssertionError("accepted duplicate received new no-dispatch refusal");}
            };
            f.dispatcher.receive(UID,BINDER,apply,receiving); // Admitted and queued, no ACK to client.
            f.engine.onApply=()->f.dispatcher.receive(UID,BINDER,apply,receiving); // Actual active dispatcher frame.
            f.engine.pump();
            require(f.engine.applies==1&&held(clientStore,ROOT),"lost reply repeated or settled apply");
            drop.set(false);f.dispatcher.receive(UID,BINDER,apply,receiving); // Consumed observation, exact original result.
            require(f.engine.applies==1&&held(clientStore,ROOT)==partial,"partial/complete original result polarity lost");
        }
    }
    static void parentDirectorySyncPrecedesPossibleDispatch()throws Exception {
        try(Fixture f=new Fixture()){
            FilesStore records=new FilesStore(f.directory.toFile(),"client");Files.createDirectories(records.directory);
            AtomicInteger sends=new AtomicInteger();Client caller=new Client(UID,WORKER,ROOT,NONCE,1,records,()->true,r->sends.incrementAndGet());
            java.util.Set<java.nio.file.attribute.PosixFilePermission> original=Files.getPosixFilePermissions(f.directory);
            boolean refused=false;
            try {
                Files.setPosixFilePermissions(f.directory,java.nio.file.attribute.PosixFilePermissions.fromString("-wx------"));
                boolean instrument=false;
                try(java.nio.channels.FileChannel parent=java.nio.channels.FileChannel.open(f.directory,StandardOpenOption.READ)){}
                catch(AccessDeniedException expected){instrument=true;}
                require(instrument,"parent-read negative unavailable under this uid");
                try{caller.send(request("parent-not-durable",Action.APPLY,3));}
                catch(IOException expected){refused=true;}
            }finally{Files.setPosixFilePermissions(f.directory,original);}
            require(refused&&sends.get()==0,"parent directory was not synced before possible dispatch");
            require(!held(records,ROOT),"pre-dispatch parent refusal invented effect hold");
            caller.send(request("parent-sync-positive",Action.APPLY,3));
            require(sends.get()==1&&held(records,ROOT),"ordinary private parent positive refused or forgot possible dispatch");
        }
    }
    public static void main(String[] args)throws Exception {
        if(args.length==1&&args[0].equals("parent-sync")){parentDirectorySyncPrecedesPossibleDispatch();System.out.println("parent durability controls passed");return;}
        if(args.length==1&&args[0].equals("duplicates")){duplicatePendingPartialAndLostReplyNeverInventNoEffect();System.out.println("duplicate controls passed");return;}
        fixedAuthorityDuplicateAndStaleControls();queuedCloseAndEpochRefuseBeforeWork();
        clientPersistenceAndReplyAuthentication();readonlyPolarityAndUnknownHistory();exactResultLookupWithoutNativeReplay();duplicatePendingPartialAndLostReplyNeverInventNoEffect();parentDirectorySyncPrecedesPossibleDispatch();
        System.out.println("creator_recovery_dispatcher controls=7 passed; actual private file persistence; no native/device effect");
    }
}
