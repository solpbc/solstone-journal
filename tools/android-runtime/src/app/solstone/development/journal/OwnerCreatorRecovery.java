// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import android.app.AlertDialog;
import android.os.*;
import android.os.Process;
import org.json.*;
import java.io.*;
import java.util.*;
import java.util.concurrent.*;
import java.util.concurrent.atomic.AtomicLong;

/** Native stopped-owner controls only. This never invokes Start or a creator. */
final class OwnerCreatorRecovery {
    final OwnerActivity owner;
    final AtomicLong choice=new AtomicLong();
    volatile Session session;
    volatile boolean busy;
    volatile CreatorRecoveryState.Result plan;
    OwnerCreatorRecovery(OwnerActivity owner){this.owner=owner;}
    CreatorRecoveryState.FilesStore store(){return new CreatorRecoveryState.FilesStore(owner.getFilesDir(),"creator-recovery-client");}
    boolean startAllowed(){
        if(busy)return false;
        try{return !CreatorRecoveryState.held(store(),owner.root)
            &&!CreatorRecoveryState.held(new CreatorRecoveryState.FilesStore(owner.getFilesDir(),"creator-recovery-service"),owner.root);}
        catch(Exception unknown){return false;}
    }
    boolean ready(){return !owner.destroyed&&owner.ownerStopped&&!owner.stopChoiceUnsaved&&owner.restoreOwnerStop()
        &&"stopped".equals(owner.runState)&&owner.worker!=null;}
    void changed(){choice.incrementAndGet();plan=null;Session current=session;session=null;if(current!=null)current.close();}
    void stopping(){choice.incrementAndGet();plan=null;
        try{owner.requests.execute(()->{try{current().stop();busy=false;refresh("");}catch(Exception unknown){refresh("stopping storage recovery · keep this app open");}});}catch(Exception unknown){}
    }
    void refresh(String reason){owner.ui.post(()->{if(!owner.destroyed&&owner.ownerStopped){owner.reason=reason;owner.renderBar();}});}
    Session current()throws Exception {
        Session current=session;
        if(current==null||!current.current()) {if(current!=null)current.close();current=new Session();session=current;}
        return current;
    }
    void review(){
        if(!ready()||busy)return;
        final long action=choice.incrementAndGet();plan=null;busy=true;refresh("reviewing storage recovery…");
        owner.requests.execute(()->{
            try {
                Session current=current();current.stop();
                Response observed=current.call(current.request(CreatorRecoveryState.Action.OBSERVE,null,null),action);
                if(observed.result==null||!observed.saved||observed.result.confirmation==null)throw new IOException("review incomplete");
                CreatorRecoveryState.Result reviewed=observed.result;
                if(!current.current()||choice.get()!=action)throw new IOException("review changed");
                plan=reviewed;busy=false;refresh("");
                owner.ui.post(()->showReview(current,reviewed,action));
            }catch(Exception unknown){busy=true;refresh("storage review is unconfirmed · use stop recovery to finish waiting");}
        });
    }
    void showReview(Session current,CreatorRecoveryState.Result reviewed,long action){
        if(!ready()||!current.current()||plan!=reviewed||choice.get()!=action)return;
        try {
            JSONObject confirmation=new JSONObject(reviewed.confirmation);int count=confirmation.getJSONArray("stages").length();
            AlertDialog.Builder dialog=new AlertDialog.Builder(owner).setTitle("review storage recovery")
                .setMessage(count==0?"no temporary recovery items remain. this check does not tell us whether an earlier archive request completed."
                    :"temporary recovery items: "+count+". copied files have matching originals. empty temporary folders may also be removed. this does not tell us whether an earlier archive request completed.")
                .setNegativeButton("keep stopped",null);
            if(count>0)dialog.setPositiveButton("remove temporary items",(d,w)->apply(current,reviewed,action));
            else {
                CreatorRecoveryState.Record prior=firstHold();
                if(prior!=null)dialog.setPositiveButton("check current storage",(d,w)->reconcile(current,prior,action));
            }
            dialog.show();
        }catch(Exception refused){refresh("storage review is unavailable · keep your journal stopped");}
    }
    CreatorRecoveryState.Record firstHold()throws Exception {
        List<CreatorRecoveryState.Record> all=store().all();
        for(CreatorRecoveryState.Record old:all) {
            if(old.request.root.equals(owner.root)&&old.request.action==CreatorRecoveryState.Action.APPLY
                    &&old.phase==CreatorRecoveryState.Phase.SENT&&(old.result==null||!old.result.noEffect&&!old.result.complete)) {
                boolean reconciled=false;
                for(CreatorRecoveryState.Record later:all)if(later.request.action==CreatorRecoveryState.Action.RECONCILE
                    &&old.request.id.equals(later.request.priorId)&&CreatorRecoveryState.digest(old.request.body).equals(later.request.priorDigest)
                    &&later.result!=null&&later.result.reconciled)reconciled=true;
                if(!reconciled)return old;
            }
        }return null;
    }
    void apply(Session current,CreatorRecoveryState.Result reviewed,long action){
        if(!ready()||busy||plan!=reviewed||choice.get()!=action||!current.current())return;
        busy=true;refresh("removing temporary recovery items…");
        owner.requests.execute(()->{
            try {
                Response response=current.call(current.request(CreatorRecoveryState.Action.APPLY,reviewed,null),action);
                busy=false;plan=null;
                if(response.result!=null&&response.saved&&response.result.complete)
                    refresh(startAllowed()?"temporary recovery items removed · your journal is still stopped. start it separately when you are ready."
                        :"temporary recovery items removed · an earlier local result is still unknown. keep stopped and check current storage.");
                else refresh("removal is incomplete or unconfirmed · keep stopped and look up saved result");
            }catch(Exception unknown){busy=true;refresh("removal outcome is unknown · use stop recovery, then look up saved result");}
        });
    }
    void reconcile(Session current,CreatorRecoveryState.Record prior,long action){
        if(!ready()||busy||choice.get()!=action||!current.current())return;
        busy=true;refresh("checking current local state…");
        owner.requests.execute(()->{
            try {
                CreatorRecoveryState.Result footprint=new CreatorRecoveryState.Result("",prior.request.id,prior.request.rootObservation,
                    prior.request.epoch,prior.request.confirmation,prior.request.revision,false,false,false,false);
                Response response=current.call(current.request(CreatorRecoveryState.Action.RECONCILE,footprint,prior.request),action);
                busy=false;plan=null;
                refresh(response.result!=null&&response.saved&&response.result.reconciled
                    ?"current local storage checked · the earlier request and remote outcome remain unknown. your journal is still stopped."
                    :"current local storage could not be confirmed · keep your journal stopped");
            }catch(Exception unknown){busy=true;refresh("current local storage is unconfirmed · use stop recovery before another review");}
        });
    }
    void lookup(){
        if(!ready()||busy)return;
        busy=true;final long action=choice.incrementAndGet();
        owner.requests.execute(()->{
            try {
                CreatorRecoveryState.Record prior=firstHold();if(prior==null){busy=false;refresh("");return;}
                Session current=current();current.stop();
                Response result=current.call(current.request(CreatorRecoveryState.Action.STATUS,null,prior.request),action);
                busy=false;
                refresh(result.result==null||!result.subject.same(prior.request)?"the earlier local result is still unknown · no request was repeated"
                    :result.saved?"the original local result was recovered · review current storage separately. no request was repeated."
                    :"the original local result could not be saved · keep your journal stopped");
            }catch(Exception unknown){busy=true;refresh("result lookup is unconfirmed · use stop recovery before another review");}
        });
    }
    static final class Response {
        final CreatorRecoveryState.Request subject;final CreatorRecoveryState.Result result;final boolean saved;
        Response(CreatorRecoveryState.Request subject,CreatorRecoveryState.Result result,boolean saved){this.subject=subject;this.result=result;this.saved=saved;}
    }
    final class Session {
        final Messenger target;final String root,nonce=UUID.randomUUID().toString();final long generation;
        final CreatorRecoveryState.Client client;final Messenger replies;
        final ConcurrentHashMap<String,Wait> waits=new ConcurrentHashMap<>();
        volatile boolean closed;volatile long stoppedEpoch=-1;
        Session()throws Exception {
            target=owner.worker;generation=owner.connectionGeneration.get();root=owner.root;
            if(target==null)throw new EOFException("worker unavailable");
            replies=new Messenger(new Handler(Looper.getMainLooper()){
                public void handleMessage(Message message){receive(message);}
            });
            client=new CreatorRecoveryState.Client(Process.myUid(),target.getBinder(),root,nonce,generation,store(),this::current,r->{
                Wait wait=waits.get(r.id);if(wait==null)throw new IOException("request waiter unavailable");
                byte[] encoded=r.encoded();if(encoded.length>CreatorRecoveryState.LIMIT)throw new IOException("request exceeds bound");
                Message message=Message.obtain(null,1);message.arg1=wait.id;message.replyTo=replies;
                Bundle data=new Bundle();data.putString("op",CreatorRecoveryState.MESSAGE);data.putByteArray("request",encoded);message.setData(data);target.send(message);
            });
        }
        boolean current(){return !closed&&owner.currentConnection(generation,target)&&root.equals(owner.root)
            &&owner.ownerStopped&&!owner.stopChoiceUnsaved&&owner.restoreOwnerStop();}
        CreatorRecoveryState.Request request(CreatorRecoveryState.Action action,CreatorRecoveryState.Result observed,CreatorRecoveryState.Request prior)throws Exception {
            String id=UUID.randomUUID().toString();boolean effect=action==CreatorRecoveryState.Action.APPLY||action==CreatorRecoveryState.Action.RECONCILE;
            String observationId=action==CreatorRecoveryState.Action.RECONCILE?prior.observationId:effect?observed.requestId:null;
            CreatorRecoveryState.Request shell=new CreatorRecoveryState.Request(id,nonce,generation,root,action,"",effect?observed.rootObservation:null,
                observationId,effect?observed.epoch:stoppedEpoch,effect?observed.confirmation:null,effect?observed.revision:null,
                prior==null?null:prior.id,prior==null?null:CreatorRecoveryState.digest(prior.body),action==CreatorRecoveryState.Action.RECONCILE?prior.body:null);
            return new CreatorRecoveryState.Request(id,nonce,generation,root,action,CreatorRecoveryEndpoint.command(shell,stoppedEpoch),shell.rootObservation,
                shell.observationId,shell.epoch,shell.confirmation,shell.revision,shell.priorId,shell.priorDigest,shell.priorBody);
        }
        Response call(CreatorRecoveryState.Request r,long action)throws Exception {
            Wait wait=new Wait(owner.sequence.incrementAndGet());waits.put(r.id,wait);
            try {
                client.send(r,()->choice.get()==action);
                return wait.future.get(90,TimeUnit.SECONDS);
            }finally{waits.remove(r.id);}
        }
        void stop()throws Exception {
            // This is the same fixed channel. Native Stop delivers cancellation
            // first, and returns only after the existing serial native join.
            Response result=call(request(CreatorRecoveryState.Action.STOP,null,null),choice.get());
            if(result.result==null||!result.saved||!result.result.stopped)throw new IOException("native Stop not joined");
            stoppedEpoch=result.result.epoch;
        }
        void receive(Message message){
            Bundle data=message.getData();String query=data.getString("query");Wait wait=waits.get(query);
            if(wait==null||message.arg1!=wait.id||message.replyTo==null)return;
            try {
                byte[] subjectBytes=data.getByteArray("subject"),resultBytes=data.getByteArray("result");
                if(subjectBytes==null||subjectBytes.length+(resultBytes==null?0:resultBytes.length)>CreatorRecoveryState.LIMIT)return;
                CreatorRecoveryState.Request subject=CreatorRecoveryState.Request.decode(subjectBytes);
                CreatorRecoveryState.Result result=resultBytes==null?null:CreatorRecoveryState.Result.decode(resultBytes);
                boolean saved=data.getBoolean("persisted");
                if(client.accept(message.sendingUid,message.replyTo.getBinder(),query,data.getString("nonce"),data.getLong("connection"),data.getString("root"),subject,result,saved))
                    wait.future.complete(new Response(subject,result,saved));
            }catch(Exception refused){wait.future.completeExceptionally(refused);}
        }
        void close(){
            if(closed)return;
            try {
                // No wait on UI destruction. Generic owner Stop already supplies
                // immediate cancellation if a recovery frame is still active.
                CreatorRecoveryState.Request close=request(CreatorRecoveryState.Action.CLOSE,null,null);
                Wait wait=new Wait(owner.sequence.incrementAndGet());waits.put(close.id,wait);client.send(close);
            }catch(Exception unknown){/* Preserve receiving records; never treat this as joined. */}
            closed=true;for(Wait wait:waits.values())wait.future.completeExceptionally(new EOFException("recovery connection closed"));waits.clear();
        }
    }
    static final class Wait {final int id;final CompletableFuture<Response> future=new CompletableFuture<>();Wait(int id){this.id=id;}}
}
