// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import java.io.*;
import java.nio.channels.FileChannel;
import java.nio.charset.StandardCharsets;
import java.nio.file.*;
import java.security.MessageDigest;
import java.util.*;

/** Private receiving records and fixed dispatch continuity. No filesystem authority. */
final class CreatorRecoveryState {
    static final String MESSAGE="owner-creator-recovery";
    static final int LIMIT=256*1024, RECORDS=128;
    enum Action { OBSERVE, APPLY, RECONCILE, STATUS, STOP, CLOSE }
    enum Phase { PREPARED, SENT, NOT_SENT }
    static boolean validId(String id) {return id!=null && id.matches("[A-Za-z0-9_-]{1,128}");}
    static boolean genericAllowed(String operation) {return !"creator-recovery".equals(operation);}
    static String digest(String text) throws Exception {
        byte[] bytes=MessageDigest.getInstance("SHA-256").digest(text.getBytes(StandardCharsets.UTF_8));
        StringBuilder hex=new StringBuilder();for(byte b:bytes)hex.append(String.format(Locale.ROOT,"%02x",b&255));return hex.toString();
    }
    static final class Request {
        final String id,nonce,root,body,rootObservation,observationId,confirmation,revision,priorId,priorDigest,priorBody;
        final long connection,epoch;
        final Action action;
        Request(String id,String nonce,long connection,String root,Action action,String body,
                String rootObservation,String observationId,long epoch,String confirmation,String revision,
                String priorId,String priorDigest) {
            this(id,nonce,connection,root,action,body,rootObservation,observationId,epoch,confirmation,revision,priorId,priorDigest,null);
        }
        Request(String id,String nonce,long connection,String root,Action action,String body,
                String rootObservation,String observationId,long epoch,String confirmation,String revision,
                String priorId,String priorDigest,String priorBody) {
            this.id=id;this.nonce=nonce;this.connection=connection;this.root=root;this.action=action;this.body=body;
            this.rootObservation=rootObservation;this.observationId=observationId;this.epoch=epoch;
            this.confirmation=confirmation;this.revision=revision;this.priorId=priorId;this.priorDigest=priorDigest;this.priorBody=priorBody;
        }
        boolean valid() {
            return validId(id)&&validId(nonce)&&connection>0&&root!=null&&root.length()<4096&&body!=null
                &&body.getBytes(StandardCharsets.UTF_8).length<=LIMIT
                &&(action!=Action.APPLY&&action!=Action.RECONCILE || validId(observationId)&&epoch>=0&&(epoch&1)==1
                    &&rootObservation!=null&&!rootObservation.isEmpty()&&confirmation!=null&&revision!=null)
                &&(action!=Action.RECONCILE||validId(priorId)&&priorDigest!=null&&priorBody!=null)
                &&(action!=Action.STATUS||priorId==null||validId(priorId));
        }
        String binding() throws Exception {return digest(new String(encoded(),StandardCharsets.ISO_8859_1));}
        byte[] encoded() throws IOException {
            ByteArrayOutputStream bytes=new ByteArrayOutputStream();DataOutputStream out=new DataOutputStream(bytes);
            out.writeInt(1);out.writeInt(action.ordinal());out.writeLong(connection);out.writeLong(epoch);
            for(String text:new String[]{id,nonce,root,body,rootObservation,observationId,confirmation,revision,priorId,priorDigest,priorBody})write(out,text);
            out.flush();if(bytes.size()>2*LIMIT)throw new IOException("recovery request exceeds bound");return bytes.toByteArray();
        }
        static Request decode(byte[] bytes) throws IOException {
            DataInputStream in=new DataInputStream(new ByteArrayInputStream(bytes));
            if(in.readInt()!=1)throw new IOException("unknown receiving schema");int a=in.readInt();
            if(a<0||a>=Action.values().length)throw new IOException("unknown action");
            long connection=in.readLong(),epoch=in.readLong();String[] s=new String[11];for(int i=0;i<s.length;i++)s[i]=read(in);
            Request request=new Request(s[0],s[1],connection,s[2],Action.values()[a],s[3],s[4],s[5],epoch,s[6],s[7],s[8],s[9],s[10]);
            if(in.available()!=0||!request.valid())throw new IOException("invalid receiving request");return request;
        }
        boolean same(Request other) throws IOException {return Arrays.equals(encoded(),other.encoded());}
    }
    /** Only the adapter decoding an actual native return constructs this. */
    static final class Result {
        final String bytes,requestId,rootObservation,confirmation,revision;
        final long epoch;
        final boolean noEffect,complete,reconciled,stopped;
        Result(String bytes,String requestId,String rootObservation,long epoch,String confirmation,String revision,
                boolean noEffect,boolean complete,boolean reconciled,boolean stopped) {
            this.bytes=bytes;this.requestId=requestId;this.rootObservation=rootObservation;this.epoch=epoch;
            this.confirmation=confirmation;this.revision=revision;this.noEffect=noEffect;this.complete=complete;
            this.reconciled=reconciled;this.stopped=stopped;
        }
        static Result refused(Request r) {return new Result("{\"not_dispatched\":true}",r.id,r.rootObservation,r.epoch,null,null,true,false,false,false);}
        boolean matches(Request r) {
            return r.id.equals(requestId)&&bytes!=null&&bytes.getBytes(StandardCharsets.UTF_8).length<=LIMIT
                &&(r.rootObservation==null||r.rootObservation.equals(rootObservation));
        }
        byte[] encoded() throws IOException {
            ByteArrayOutputStream bytes=new ByteArrayOutputStream();DataOutputStream out=new DataOutputStream(bytes);
            out.writeInt(1);out.writeLong(epoch);out.writeBoolean(noEffect);out.writeBoolean(complete);out.writeBoolean(reconciled);out.writeBoolean(stopped);
            for(String text:new String[]{this.bytes,requestId,rootObservation,confirmation,revision})write(out,text);out.flush();return bytes.toByteArray();
        }
        static Result decode(byte[] bytes) throws IOException {
            DataInputStream in=new DataInputStream(new ByteArrayInputStream(bytes));if(in.readInt()!=1)throw new IOException("unknown result schema");
            long epoch=in.readLong();boolean noEffect=in.readBoolean(),complete=in.readBoolean(),reconciled=in.readBoolean(),stopped=in.readBoolean();
            String[] s=new String[5];for(int i=0;i<s.length;i++)s[i]=read(in);
            if(in.available()!=0)throw new IOException("trailing result data");return new Result(s[0],s[1],s[2],epoch,s[3],s[4],noEffect,complete,reconciled,stopped);
        }
    }
    static void write(DataOutputStream out,String text) throws IOException {
        if(text==null){out.writeInt(-1);return;}byte[] bytes=text.getBytes(StandardCharsets.UTF_8);
        if(bytes.length>LIMIT)throw new IOException("receiving field exceeds bound");out.writeInt(bytes.length);out.write(bytes);
    }
    static String read(DataInputStream in) throws IOException {
        int length=in.readInt();if(length==-1)return null;if(length<0||length>LIMIT)throw new IOException("invalid receiving length");
        byte[] bytes=new byte[length];in.readFully(bytes);return new String(bytes,StandardCharsets.UTF_8);
    }
    interface Store {
        Record get(String id) throws Exception;
        List<Record> all() throws Exception;
        void request(Request request,Phase phase) throws Exception;
        void result(Request request,Result result) throws Exception;
    }
    static final class Record {
        final Request request;final Phase phase;final Result result;
        Record(Request request,Phase phase,Result result){this.request=request;this.phase=phase;this.result=result;}
    }
    /** Normal atomic private app records, outside the journal. Reading creates nothing. */
    static final class FilesStore implements Store {
        final Path directory;
        FilesStore(File appFiles,String name) {directory=new File(appFiles,name).toPath();}
        Path path(String id,String suffix) throws IOException {
            if(!validId(id))throw new IOException("invalid receiving id");return directory.resolve(id+suffix);
        }
        byte[] existing(Path path) throws IOException {
            if(!Files.exists(path,LinkOption.NOFOLLOW_LINKS))return null;
            if(!Files.isRegularFile(path,LinkOption.NOFOLLOW_LINKS)||Files.size(path)>3*LIMIT)throw new IOException("invalid receiving record");
            return Files.readAllBytes(path);
        }
        synchronized public Record get(String id) throws Exception {
            byte[] bytes=existing(path(id,".request"));if(bytes==null)return null;
            DataInputStream in=new DataInputStream(new ByteArrayInputStream(bytes));int phase=in.readInt();int length=in.readInt();
            if(phase<0||phase>=Phase.values().length||length<0||length>2*LIMIT)throw new IOException("invalid receiving record");
            byte[] request=new byte[length];in.readFully(request);if(in.available()!=0)throw new IOException("trailing receiving data");
            Request r=Request.decode(request);if(!r.id.equals(id))throw new IOException("receiving id changed");
            byte[] result=existing(path(id,".result"));Result value=null;
            if(result!=null){DataInputStream saved=new DataInputStream(new ByteArrayInputStream(result));
                if(!r.binding().equals(read(saved)))throw new IOException("result request changed");
                int size=saved.readInt();if(size<0||size>2*LIMIT)throw new IOException("invalid result size");
                byte[] payload=new byte[size];saved.readFully(payload);if(saved.available()!=0)throw new IOException("trailing result data");value=Result.decode(payload);}
            if(value!=null&&!value.matches(r))throw new IOException("receiving result changed");
            return new Record(r,Phase.values()[phase],value);
        }
        synchronized public List<Record> all() throws Exception {
            List<Record> records=new ArrayList<>();if(!Files.exists(directory,LinkOption.NOFOLLOW_LINKS))return records;
            if(!Files.isDirectory(directory,LinkOption.NOFOLLOW_LINKS))throw new IOException("invalid receiving directory");
            try(DirectoryStream<Path> files=Files.newDirectoryStream(directory,"*.request")) {
                for(Path file:files){if(records.size()>=RECORDS)throw new IOException("receiving history exceeds bound");
                    String name=file.getFileName().toString();records.add(get(name.substring(0,name.length()-8)));}
            }return records;
        }
        void atomic(Path target,byte[] bytes) throws Exception {
            Files.createDirectories(directory);if(Files.isSymbolicLink(directory))throw new IOException("invalid receiving directory");
            // Persist the receiving directory's entry too, before a request can
            // become possibly dispatched. File + child-directory sync alone
            // does not seal a newly created directory in its app-files parent.
            try(FileChannel parent=FileChannel.open(directory.getParent(),StandardOpenOption.READ)){parent.force(true);}
            Path tmp=Files.createTempFile(directory,"receiving-",".tmp");
            try {
                try(FileOutputStream output=new FileOutputStream(tmp.toFile())){output.write(bytes);output.getFD().sync();}
                Files.move(tmp,target,StandardCopyOption.ATOMIC_MOVE,StandardCopyOption.REPLACE_EXISTING);
                try(FileChannel parent=FileChannel.open(directory,StandardOpenOption.READ)){parent.force(true);}
            }finally{Files.deleteIfExists(tmp);}
        }
        synchronized public void request(Request r,Phase phase) throws Exception {
            Record previous=get(r.id);if(previous!=null&&!previous.request.same(r))throw new IOException("request id reused");
            if(previous==null&&all().size()>=RECORDS)throw new IOException("receiving history exceeds bound");
            if(previous!=null&&previous.phase==Phase.SENT&&phase!=Phase.SENT)throw new IOException("cannot forget possible dispatch");
            ByteArrayOutputStream bytes=new ByteArrayOutputStream();DataOutputStream out=new DataOutputStream(bytes);
            byte[] encoded=r.encoded();out.writeInt(phase.ordinal());out.writeInt(encoded.length);out.write(encoded);out.flush();atomic(path(r.id,".request"),bytes.toByteArray());
        }
        synchronized public void result(Request r,Result result) throws Exception {
            Record previous=get(r.id);if(previous==null||!previous.request.same(r)||!result.matches(r))throw new IOException("unbound receiving result");
            if(previous.result!=null&&!Arrays.equals(previous.result.encoded(),result.encoded()))throw new IOException("result cannot change");
            ByteArrayOutputStream bytes=new ByteArrayOutputStream();DataOutputStream out=new DataOutputStream(bytes);
            write(out,r.binding());byte[] payload=result.encoded();out.writeInt(payload.length);out.write(payload);out.flush();
            atomic(path(r.id,".result"),bytes.toByteArray());
        }
    }
    static boolean held(Store store,String root) throws Exception {
        List<Record> records=store.all();
        for(Record record:records) {
            Request request=record.request;
            if(!request.root.equals(root)||request.action!=Action.APPLY||record.phase!=Phase.SENT)continue;
            if(record.result!=null&&(record.result.noEffect||record.result.complete))continue;
            boolean reconciled=false;
            for(Record later:records)if(later.request.root.equals(root)&&later.request.action==Action.RECONCILE&&request.id.equals(later.request.priorId)
                    &&digest(request.body).equals(later.request.priorDigest)&&later.result!=null&&later.result.reconciled)reconciled=true;
            if(!reconciled)return true;
        }return false;
    }
    interface Host {
        long epoch() throws Exception;
        void cancel();
        void execute(Runnable work) throws Exception;
        Result run(Request request) throws Exception;
    }
    interface Receiver {void returned(Request request,Result result,boolean persisted);void pending(Request request);void refused(Request request);}
    static final class Scope {
        final Object endpoint;final String nonce,root;final long generation;boolean live=true;long stoppedEpoch=-1;
        Request observationRequest;Result observation;
        Scope(Object endpoint,Request r){this.endpoint=endpoint;nonce=r.nonce;root=r.root;generation=r.connection;}
        boolean matches(Object endpoint,Request r){return live&&this.endpoint.equals(endpoint)&&nonce.equals(r.nonce)&&root.equals(r.root)&&generation==r.connection;}
    }
    /** The production fixed dispatcher. The Android adapter supplies actual Binder identity. */
    static final class Dispatcher {
        final int uid;final String root;final Store store;final Host host;
        Scope scope;final Map<String,Result> returned=new HashMap<>();final Set<String> reserved=new HashSet<>();
        int active;
        Dispatcher(int uid,String root,Store store,Host host){this.uid=uid;this.root=root;this.store=store;this.host=host;}
        boolean ordinaryStartAllowed(){try{synchronized(this){if(active!=0)return false;}return !held(store,root);}catch(Exception refused){return false;}}
        void close(Object endpoint) {boolean cancel=false;synchronized(this){if(scope!=null&&scope.endpoint.equals(endpoint)){scope.live=false;cancel=active!=0;}}if(cancel)host.cancel();}
        void receive(int actualUid,Object endpoint,Request r,Receiver reply) {
            boolean ownReservation=false,queued=false;
            try {
                if(actualUid!=uid||endpoint==null||!r.valid()||!root.equals(r.root))return;
                if(r.action==Action.CLOSE){synchronized(this){if(scope==null||!scope.matches(endpoint,r)){reply.refused(r);return;}}close(endpoint);reply.returned(r,Result.refused(r),true);return;}
                final Scope owner;
                synchronized(this) {
                    if(scope==null||!scope.matches(endpoint,r)) {
                        if(r.action!=Action.STOP){reply.refused(r);return;}
                        if(scope!=null)scope.live=false;
                        scope=new Scope(endpoint,r);
                    }owner=scope;
                }
                if(r.action==Action.STATUS&&r.priorId!=null){lookup(owner,r,reply);return;}
                // An accepted request is history, not a new admission. Resolve it
                // BEFORE consumed confirmation, active-frame or epoch refusal;
                // otherwise a duplicate could falsely settle an unknown apply.
                Record accepted=store.get(r.id);
                if(accepted!=null){
                    if(!accepted.request.same(r)){reply.pending(r);return;}
                    lookup(owner,r,reply,r.id);return;
                }
                synchronized(this){if(reserved.contains(r.id)){reply.pending(r);return;}}
                if(r.action==Action.STOP)host.cancel();
                long epoch=host.epoch();
                synchronized(this) {
                    if(!owner.live||scope!=owner){reply.refused(r);return;}
                    if(r.action!=Action.STOP&&(owner.stoppedEpoch!=epoch||(epoch&1)!=1||active!=0)) {reply.refused(r);return;}
                    if(r.action==Action.APPLY&&(!observed(owner,r,epoch)
                        ||r.encoded().length+owner.observation.encoded().length+32*1024>LIMIT)){reply.refused(r);return;}
                    if(r.action==Action.RECONCILE) {
                        Record prior=store.get(r.priorId);
                        if(!digest(r.priorBody).equals(r.priorDigest)
                            ||prior!=null&&(prior.request.action!=Action.APPLY||!prior.request.root.equals(root)
                                ||!prior.request.body.equals(r.priorBody)||!Objects.equals(prior.request.confirmation,r.confirmation)
                                ||!Objects.equals(prior.request.revision,r.revision)||!Objects.equals(prior.request.rootObservation,r.rootObservation))) {reply.refused(r);return;}
                        // If service pre-admission persistence never succeeded, a
                        // client may still have uncertain dispatch. The adapter
                        // validates its complete prior native body; only NEW
                        // read-only native proof can settle that local hold.
                    }
                    if(reserved.contains(r.id)){reply.pending(r);return;}
                    reserved.add(r.id);ownReservation=true;
                }
                Record prior=store.get(r.id);
                if(prior!=null){if(!prior.request.same(r)){reply.refused(r);return;}lookup(owner,r,reply,r.id);return;}
                // Persist possible admission BEFORE enqueue. A lost ACK cannot erase it.
                store.request(r,Phase.SENT);
                boolean current;synchronized(this){current=owner.live&&scope==owner;}
                if(!current){finish(r,Result.refused(r),reply);return;}
                try {host.execute(()->run(owner,r,reply));queued=true;}
                catch(Exception rejected){finish(r,Result.refused(r),reply);}
            } catch(Exception unknown){reply.pending(r);}
            finally{if(ownReservation&&!queued)synchronized(this){reserved.remove(r.id);}}
        }
        boolean observed(Scope owner,Request r,long epoch) {
            return owner.observation!=null&&owner.observationRequest!=null&&epoch==r.epoch
                &&owner.observationRequest.id.equals(r.observationId)&&Objects.equals(owner.observation.confirmation,r.confirmation)
                &&Objects.equals(owner.observation.revision,r.revision)&&Objects.equals(owner.observation.rootObservation,r.rootObservation);
        }
        void run(Scope owner,Request r,Receiver reply) {
            boolean admitted=false;
            try {
                long epoch=host.epoch();
                boolean current;
                synchronized(this) {
                    current=owner.live&&scope==owner&&(r.action==Action.STOP||owner.stoppedEpoch==epoch&&(epoch&1)==1)
                        &&(r.action!=Action.APPLY||observed(owner,r,epoch));
                    if(current){active++;admitted=true;}
                }
                if(!current){finish(r,Result.refused(r),reply);return;}
                Result result=host.run(r);
                if(!result.matches(r))throw new IOException("native result binding refused");
                synchronized(this) {
                    if(owner.live&&scope==owner) {
                        if(r.action==Action.STOP&&result.stopped)owner.stoppedEpoch=result.epoch;
                        if(r.action==Action.OBSERVE&&result.confirmation!=null&&result.epoch==owner.stoppedEpoch) {
                            owner.observationRequest=r;owner.observation=result;
                        }
                        if(r.action==Action.APPLY)owner.observation=null;
                    }
                }
                finish(r,result,reply);
            } catch(Exception unknown){reply.pending(r);}
            finally{synchronized(this){if(admitted)active--;reserved.remove(r.id);}}
        }
        void finish(Request r,Result result,Receiver reply) {
            synchronized(this){returned.put(r.id,result);}
            boolean saved=false;try{store.result(r,result);saved=true;}catch(Exception unknown){/* Preserve possible dispatch and actual in-memory facts. */}
            reply.returned(r,result,saved);
        }
        void lookup(Scope owner,Request r,Receiver reply) throws Exception {lookup(owner,r,reply,r.priorId);}
        void lookup(Scope owner,Request r,Receiver reply,String id) throws Exception {
            Record record=store.get(id);if(record==null||!record.request.root.equals(root)
                ||r.priorId!=null&&!digest(record.request.body).equals(r.priorDigest)){reply.refused(r);return;}
            Result value; synchronized(this){if(!owner.live||scope!=owner){reply.refused(r);return;}value=returned.get(id);}
            if(value==null)value=record.result;
            if(value==null){reply.pending(record.request);return;}
            boolean saved=false;try{store.result(record.request,value);saved=true;}catch(Exception unknown){}
            reply.returned(record.request,value,saved);
        }
    }
    interface Guard {boolean current();}
    interface Sender {void send(Request request) throws Exception;}
    static final class Client {
        final int uid;final Object worker;final String root,nonce;final long connection;
        final Store store;final Guard guard;final Sender sender;
        final Map<String,Request> queries=new HashMap<>();final Map<String,Result> received=new HashMap<>();
        Client(int uid,Object worker,String root,String nonce,long connection,Store store,Guard guard,Sender sender) {
            this.uid=uid;this.worker=worker;this.root=root;this.nonce=nonce;this.connection=connection;
            this.store=store;this.guard=guard;this.sender=sender;
        }
        void send(Request r) throws Exception {send(r,()->true);}
        void send(Request r,Guard dispatchGuard) throws Exception {
            if(!dispatchGuard.current()||!r.valid()||!r.root.equals(root)||!r.nonce.equals(nonce)||r.connection!=connection||!guard.current())
                throw new IOException("stale recovery caller");
            // The immutable request exists before any possible dispatch.
            store.request(r,Phase.PREPARED);
            if(!guard.current()||!dispatchGuard.current())throw new IOException("caller changed during persistence");
            store.request(r,Phase.SENT);
            if(!guard.current()||!dispatchGuard.current()) {
                // This is an actual no-send observation, never a timeout inference.
                store.result(r,Result.refused(r));throw new IOException("caller changed before dispatch");
            }
            synchronized(this){queries.put(r.id,r);}
            sender.send(r); // A throw/lost ACK retains SENT; never retry here.
        }
        boolean accept(int actualUid,Object replyWorker,String query,String replyNonce,long replyConnection,String replyRoot,
                Request subject,Result result,boolean servicePersisted) throws Exception {
            if(actualUid!=uid||!worker.equals(replyWorker)||!nonce.equals(replyNonce)||connection!=replyConnection
                ||!root.equals(replyRoot)||!guard.current())return false;
            Request requested;synchronized(this){requested=queries.get(query);}
            if(requested==null)return false;
            Record original=store.get(subject.id);
            if(original==null||!original.request.same(subject)||!subject.root.equals(root))return false;
            if(!requested.id.equals(subject.id)&&(requested.action!=Action.STATUS||!subject.id.equals(requested.priorId)
                    ||!digest(subject.body).equals(requested.priorDigest)))return false;
            if(result==null)return true; // Pending is not completion or Stop.
            if(!result.matches(subject))return false;
            synchronized(this){received.put(subject.id,result);}
            // A failed service or client save cannot clear the durable unknown hold.
            if(servicePersisted)store.result(subject,result);
            synchronized(this){queries.remove(query);}
            return true;
        }
        boolean held() {try{return CreatorRecoveryState.held(store,root);}catch(Exception unknown){return true;}}
    }

}
