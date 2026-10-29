// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import android.app.AlertDialog;
import android.os.*;
import java.nio.file.*;
import java.util.*;
import java.util.concurrent.*;
import static app.solstone.development.journal.CreatorRecoveryState.*;
import app.solstone.development.journal.CreatorRecoveryState.Record;

/** Uses the maintained endpoint, receiving store, caller and Review path. */
public final class CreatorRecoveryDeliveryTest {
    static void require(boolean value,String why) { if(!value)throw new AssertionError(why); }
    static final class Fixture implements AutoCloseable {
        final Path temp=Files.createTempDirectory("recovery-delivery-");
        final OwnerActivity owner=new OwnerActivity(temp.toFile());
        final JournalService service=new JournalService(temp.toFile());
        final OwnerCreatorRecovery recovery=new OwnerCreatorRecovery(owner);
        Fixture() throws Exception { Native.epoch=1;Native.stops=0;Native.observes=0;AlertDialog.shown=0;AlertDialog.displayed=new CountDownLatch(1);owner.worker=service.incoming; }
        FilesStore serviceStore() { return new FilesStore(temp.toFile(),"creator-recovery-service"); }
        public void close() throws Exception {
            recovery.changed();owner.requests.shutdownNow();
            require(owner.requests.awaitTermination(5,TimeUnit.SECONDS),"caller threads did not join");
            try(java.util.stream.Stream<Path> files=Files.walk(temp)) {
                for(Path file:files.sorted(Comparator.reverseOrder()).toArray(Path[]::new))Files.delete(file);
            }
        }
    }
    static void sameStopAfterRecycling(int recycledId) throws Exception {
        try(Fixture f=new Fixture()) {
            f.service.recycledId=recycledId;
            OwnerCreatorRecovery.Session session=f.recovery.current();
            Future<?> stopped=f.owner.requests.submit(()->{try{session.stop();}catch(Exception error){throw new RuntimeException(error);}});
            f.service.runNext();
            Record saved=f.serviceStore().all().get(0);
            require(saved.result!=null&&saved.result.stopped&&!saved.result.complete,"synthetic service Stop fact missing");
            Record received=f.recovery.store().get(saved.request.id);
            require(received.result!=null,"service saved Stop but owner dropped recycled-Message reply: arg1="+f.service.lastReply.arg1);
            require(Arrays.equals(saved.result.encoded(),received.result.encoded()),"owner did not save identical Stop bytes");
            stopped.get(5,TimeUnit.SECONDS);
            require(session.stoppedEpoch==saved.result.epoch,"owner did not acquire actual stopped epoch");
            require(Native.stops==1&&Native.observes==0,"Stop invoked extra work");
            System.out.println("PASS recycled request: service Stop equals client Stop; complete=false accepted; joined epoch="+session.stoppedEpoch);
        }
    }
    static void reviewAndRebind() throws Exception {
        try(Fixture f=new Fixture()) {
            f.recovery.review();f.service.runNext();f.service.runNext();
            OwnerCreatorRecovery.Session old=f.recovery.current();
            require(AlertDialog.displayed.await(5,TimeUnit.SECONDS),"Review dialog did not arrive");
            require(f.recovery.plan!=null&&!f.recovery.busy&&AlertDialog.shown==1,"actual Review did not show read-only inspection");
            require(Native.stops==1&&Native.observes==1,"Review automatically invoked extra work");
            f.recovery.changed();f.owner.connectionGeneration.incrementAndGet();
            f.owner.worker=f.service.incoming;
            OwnerCreatorRecovery.Session fresh=f.recovery.current();
            require(fresh.stoppedEpoch==-1,"rebind inherited old Stop authority");
            require(!old.current()&&fresh.current(),"rebind connection guard wrong");
            Record original=f.recovery.store().all().stream().filter(r->r.request.action==Action.STOP).findFirst().get();
            require(original.result!=null&&original.result.stopped,"rebind erased durable Stop fact");
            Future<?> stopped=f.owner.requests.submit(()->{try{fresh.stop();}catch(Exception error){throw new RuntimeException(error);}});
            f.service.runNext();stopped.get(5,TimeUnit.SECONDS);
            require(fresh.stoppedEpoch>original.result.epoch,"fresh channel did not join a new Stop");
            require(Native.stops==2&&Native.observes==1,"rebind replayed Review or started work");
            System.out.println("PASS actual Review: STOP -> OBSERVE -> dialog; no apply or Start; rebind preserves facts but requires fresh Stop");
        }
    }
    static void lostReply() throws Exception {
        try(Fixture f=new Fixture()) {
            f.service.dropReply=true;OwnerCreatorRecovery.Session session=f.recovery.current();
            Future<?> stopped=f.owner.requests.submit(()->{try{session.stop();throw new AssertionError("lost Stop completed");}catch(Exception expected){}});
            f.service.runNext();Record saved=f.serviceStore().all().get(0);
            require(saved.result.stopped&&f.recovery.store().get(saved.request.id).result==null,"lost reply invented client completion");
            require(!stopped.isDone()&&session.stoppedEpoch==-1,"unknown Stop authorized Review");
            f.recovery.changed();stopped.get(5,TimeUnit.SECONDS);
            require(Native.stops==1&&Native.observes==0,"lost reply repeated native work");
            System.out.println("PASS lost reply: service fact retained, client unknown, no observe/replay");
        }
    }
    static Message copy(Message original) {
        Message message=Message.obtain(null,1);message.arg1=original.arg1;
        message.sendingUid=original.sendingUid;message.replyTo=original.replyTo;
        Bundle from=original.getData(),to=new Bundle();
        for(String key:new String[]{"query","nonce","root"})to.putString(key,from.getString(key));
        to.putLong("connection",from.getLong("connection"));to.putBoolean("persisted",from.getBoolean("persisted"));
        to.putByteArray("subject",from.getByteArray("subject"));to.putByteArray("result",from.getByteArray("result"));
        message.setData(to);return message;
    }
    static void mismatches() throws Exception {
        try(Fixture f=new Fixture()) {
            f.service.dropReply=true;OwnerCreatorRecovery.Session session=f.recovery.current();
            Future<?> stopped=f.owner.requests.submit(()->{try{session.stop();}catch(Exception error){throw new RuntimeException(error);}});
            f.service.runNext();Message actual=copy(f.service.lastReply);
            Record saved=f.serviceStore().all().get(0);
            for(String field:new String[]{"waiter","uid","worker","query","nonce","connection","root","subject","result"}) {
                Message bad=copy(actual);Bundle data=bad.getData();
                switch(field) {
                    case "waiter":bad.arg1++;break;
                    case "uid":bad.sendingUid++;break;
                    case "worker":bad.replyTo=new Messenger(new Handler(Looper.getMainLooper()));break;
                    case "query":data.putString(field,"foreign-query");break;
                    case "nonce":data.putString(field,"foreign-nonce");break;
                    case "connection":data.putLong(field,999);break;
                    case "root":data.putString(field,"/foreign-root");break;
                    case "subject":
                        Request r=saved.request;
                        data.putByteArray(field,new Request(r.id,r.nonce,r.connection,"/foreign-root",r.action,r.body,null,null,r.epoch,null,null,null,null).encoded());break;
                    case "result":
                        Result value=saved.result;
                        data.putByteArray(field,new Result(value.bytes,"foreign-request",value.rootObservation,value.epoch,value.confirmation,value.revision,value.noEffect,value.complete,value.reconciled,value.stopped).encoded());break;
                }
                session.receive(bad);
                require(!stopped.isDone()&&session.stoppedEpoch==-1&&f.recovery.store().get(saved.request.id).result==null,
                    "mismatched "+field+" supplied completion");
            }
            session.receive(actual);stopped.get(5,TimeUnit.SECONDS);
            require(Arrays.equals(saved.result.encoded(),f.recovery.store().get(saved.request.id).result.encoded()),"valid reply failed after mismatches");
            f.recovery.changed();f.owner.connectionGeneration.incrementAndGet();
            OwnerCreatorRecovery.Session fresh=f.recovery.current();session.receive(actual);fresh.receive(actual);
            require(!session.current()&&fresh.stoppedEpoch==-1,"stale connection result supplied new authority");
            System.out.println("PASS actual caller rejects mismatched waiter/UID/worker/query/nonce/connection/root/subject/result and stale rebind reply");
        }
    }
    public static void main(String[] args) throws Exception {
        sameStopAfterRecycling(0);sameStopAfterRecycling(84721);reviewAndRebind();lostReply();mismatches();
    }
}
