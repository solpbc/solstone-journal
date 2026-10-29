// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import android.os.*;
import android.os.Process;
import org.json.*;
import java.io.*;
import java.util.*;

/** Fixed package-owned Messenger path. No generic serve or externally supplied root. */
final class CreatorRecoveryEndpoint {
    final JournalService service;
    final CreatorRecoveryState.Dispatcher dispatcher;
    final Map<IBinder,IBinder.DeathRecipient> deaths=new HashMap<>();
    CreatorRecoveryEndpoint(JournalService service) {
        this.service=service;
        final String root=new File(service.getFilesDir(),"synthetic-serving-journal").toString();
        dispatcher=new CreatorRecoveryState.Dispatcher(Process.myUid(),root,
            new CreatorRecoveryState.FilesStore(service.getFilesDir(),"creator-recovery-service"),new CreatorRecoveryState.Host(){
                public long epoch() throws Exception {return nativeEpoch();}
                public void cancel(){service.execution.beginStop(service::cancelNativeWork);}
                public void execute(Runnable work){service.work.execute(work);}
                public CreatorRecoveryState.Result run(CreatorRecoveryState.Request request) throws Exception {
                    if(service.destroyed)throw new IOException("service closed");
                    String command=command(request,nativeEpoch());
                    if(!canonical(new JSONObject(request.body)).equals(canonical(new JSONObject(command))))
                        throw new IOException("native command changed");
                    if(request.action==CreatorRecoveryState.Action.STOP)service.thinkingAdmission.awaitIdle();
                    String result=Native.run(root,"serve:"+command,1);
                    JSONObject returned=new JSONObject(result),value=returned.optJSONObject("value");
                    if(!returned.optBoolean("ok")||value==null)
                        return new CreatorRecoveryState.Result(result,request.id,request.rootObservation,nativeEpoch(),null,null,false,false,false,false);
                    if(request.action==CreatorRecoveryState.Action.STOP||request.action==CreatorRecoveryState.Action.STATUS)
                        return new CreatorRecoveryState.Result(result,request.id,request.rootObservation,nativeEpoch(),null,null,true,false,false,
                            value.optBoolean("stopped")&&!value.optBoolean("stopping")&&!value.optBoolean("failed"));
                    if(!request.id.equals(value.optString("request_id"))||!"local_redundant_creator_copies".equals(value.optString("scope"))
                            ||value.optBoolean("replay_authorized",true)||value.optBoolean("ordinary_work_started",true)
                            ||!"unknown".equals(value.optString("remote_outcome")))throw new IOException("native result binding refused");
                    JSONObject confirmation=value.optJSONObject("confirmation");
                    return new CreatorRecoveryState.Result(result,request.id,value.getString("root_observation"),value.getLong("native_epoch"),
                        confirmation==null?null:canonical(confirmation),value.optString("confirmation_revision",null),
                        value.has("attempted")&&!value.getBoolean("attempted"),value.optBoolean("local_complete"),
                        value.optBoolean("current_local_state_reconciled"),false);
                }
            });
    }
    static long nativeEpoch() throws Exception {
        JSONObject nativeResult=new JSONObject(Native.run("","thinking-generation",1));
        if(!nativeResult.optBoolean("ok"))throw new IOException("native epoch unavailable");
        return nativeResult.getJSONObject("value").getLong("generation");
    }
    static String command(CreatorRecoveryState.Request r,long admissionEpoch) throws Exception {
        JSONObject body=new JSONObject();
        switch(r.action) {
            case STOP:body.put("op","stop");break;
            case STATUS:body.put("op",r.priorId==null?"status":"lookup");break;
            case CLOSE:body.put("op","close");break;
            default:
                body.put("op","creator-recovery");body.put("action",r.action.name().toLowerCase(Locale.ROOT));
                body.put("admission_generation",admissionEpoch);body.put("request_id",r.id);
                if(r.rootObservation!=null)body.put("root_observation",r.rootObservation);
                if(r.action!=CreatorRecoveryState.Action.OBSERVE) {
                    body.put("observation_request_id",r.observationId);body.put("observation_epoch",r.epoch);
                    body.put("confirmation",new JSONObject(r.confirmation));body.put("confirmation_revision",r.revision);
                }
        }
        return canonical(body);
    }
    static String canonical(Object value) throws JSONException {
        if(value instanceof JSONObject) {
            JSONObject object=(JSONObject)value;List<String> names=new ArrayList<>();
            Iterator<String> keys=object.keys();while(keys.hasNext())names.add(keys.next());Collections.sort(names);
            StringBuilder out=new StringBuilder("{");for(String name:names){if(out.length()>1)out.append(',');out.append(JSONObject.quote(name)).append(':').append(canonical(object.get(name)));}return out.append('}').toString();
        }
        if(value instanceof JSONArray) {JSONArray array=(JSONArray)value;StringBuilder out=new StringBuilder("[");for(int i=0;i<array.length();i++){if(i>0)out.append(',');out.append(canonical(array.get(i)));}return out.append(']').toString();}
        if(value==null||value==JSONObject.NULL)return "null";
        return value instanceof String?JSONObject.quote((String)value):value.toString();
    }
    boolean handle(Message message) {
        if(!CreatorRecoveryState.MESSAGE.equals(message.getData().getString("op")))return false;
        // The UID is supplied by Binder, never by the Bundle. Refuse before parse/register.
        if(service.destroyed||message.sendingUid!=Process.myUid()||message.replyTo==null)return true;
        final Messenger reply=message.replyTo;final IBinder endpoint=reply.getBinder();
        // The Message may be recycled before deferred native work returns.
        final int replyId=message.arg1;
        try {
            byte[] bytes=message.getData().getByteArray("request");
            if(bytes==null||bytes.length>CreatorRecoveryState.LIMIT)throw new IOException("request bound");
            final CreatorRecoveryState.Request request=CreatorRecoveryState.Request.decode(bytes);
            if(!canonical(new JSONObject(request.body)).equals(command(request,nativeEpoch())))throw new IOException("fixed request refused");
            if(request.action==CreatorRecoveryState.Action.RECONCILE) {
                JSONObject prior=new JSONObject(request.priorBody);
                if(!CreatorRecoveryState.digest(request.priorBody).equals(request.priorDigest)
                    ||!"creator-recovery".equals(prior.optString("op"))||!"apply".equals(prior.optString("action"))
                    ||!request.priorId.equals(prior.optString("request_id"))
                    ||!request.rootObservation.equals(prior.optString("root_observation"))
                    ||!request.observationId.equals(prior.optString("observation_request_id"))
                    ||request.epoch!=prior.optLong("observation_epoch",-1)||request.epoch!=prior.optLong("admission_generation",-1)
                    ||!request.confirmation.equals(canonical(prior.getJSONObject("confirmation")))
                    ||!request.revision.equals(prior.optString("confirmation_revision")))throw new IOException("prior footprint refused");
            }
            synchronized(deaths) {
                if(!deaths.containsKey(endpoint)) {
                    IBinder.DeathRecipient died=()->{dispatcher.close(endpoint);synchronized(deaths){deaths.remove(endpoint);}};
                    endpoint.linkToDeath(died,0);deaths.put(endpoint,died);
                }
            }
            dispatcher.receive(message.sendingUid,endpoint,request,new CreatorRecoveryState.Receiver(){
                public void returned(CreatorRecoveryState.Request subject,CreatorRecoveryState.Result result,boolean saved){send(subject,result,saved,false,false);}
                public void pending(CreatorRecoveryState.Request subject){send(subject,null,false,true,false);}
                public void refused(CreatorRecoveryState.Request subject){send(subject,CreatorRecoveryState.Result.refused(subject),true,false,true);}
                void send(CreatorRecoveryState.Request subject,CreatorRecoveryState.Result result,boolean saved,boolean pending,boolean refused) {
                    try {
                        Message response=Message.obtain(null,1);response.arg1=replyId;response.replyTo=service.incoming;
                        Bundle data=new Bundle();data.putString("query",request.id);data.putString("nonce",request.nonce);
                        data.putLong("connection",request.connection);data.putString("root",request.root);
                        byte[] subjectBytes=subject.encoded(),resultBytes=result==null?null:result.encoded();
                        if(subjectBytes.length+(resultBytes==null?0:resultBytes.length)>CreatorRecoveryState.LIMIT)throw new IOException("response exceeds bound");
                        data.putByteArray("subject",subjectBytes);if(resultBytes!=null)data.putByteArray("result",resultBytes);
                        data.putBoolean("persisted",saved);data.putBoolean("pending",pending);data.putBoolean("refused",refused);response.setData(data);
                        reply.send(response);
                    }catch(Exception lost){/* Exact result remains in receiving store/memory; never repeat Native. */}
                }
            });
        }catch(Exception refused){/* An unauthenticated/malformed envelope gets no completion proof. */}
        return true;
    }
    boolean ordinaryStartAllowed(){return dispatcher.ordinaryStartAllowed();}
    void close() {
        synchronized(deaths){for(Map.Entry<IBinder,IBinder.DeathRecipient> entry:deaths.entrySet()){
            dispatcher.close(entry.getKey());try{entry.getKey().unlinkToDeath(entry.getValue(),0);}catch(NoSuchElementException ignored){}
        }deaths.clear();}
    }
}
