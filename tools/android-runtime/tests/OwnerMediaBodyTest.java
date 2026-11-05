// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import java.io.*;
import java.util.Arrays;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

public final class OwnerMediaBodyTest {
    static void check(boolean value) {if(!value) throw new AssertionError();}
    static final class Pipe extends ByteArrayInputStream {
        boolean closed;
        Pipe(byte[] bytes) {super(bytes);}
        public int available() {return Math.min(65536,super.available());}
        public void close() {closed=true;}
    }
    // The relevant Chromium sequence: available -> ComputeBounds -> skip -> read.
    static byte[] chromium(InputStream stream,long first,long last) throws Exception {
        int total=stream.available();
        if(first<0) {first=Math.max(0,total+first);last=total-1;}
        else last=last<0 ? total-1 : Math.min(last,total-1);
        if(first>last || first>=total) throw new IOException("range outside available size");
        long skip=first;
        while(skip>0) {long n=stream.skip(skip);if(n<=0) throw new IOException("seek failed");skip-=n;}
        ByteArrayOutputStream result=new ByteArrayOutputStream();byte[] chunk=new byte[16384];int n;
        while((n=stream.read(chunk))!=-1) result.write(chunk,0,n);
        check(result.size()==last-first+1);
        return result.toByteArray();
    }
    static void interval(String request,int start,int end,long first,long last) throws Exception {
        byte[] full=new byte[374779];for(int i=0;i<full.length;i++) full[i]=(byte)(i%251);
        byte[] selected=Arrays.copyOfRange(full,start,end+1);Pipe source=new Pipe(selected);
        OwnerMediaBody body=OwnerMediaBody.adapt(source,request,"bytes "+start+"-"+end+"/"+full.length,""+selected.length);
        check(body!=null);check(Arrays.equals(chromium(body,first,last),selected));check(source.closed);
    }
    public static void main(String[] args) throws Exception {
        interval("bytes=0-",0,374778,0,-1);
        interval("bytes=131072-",131072,374778,131072,-1);
        interval("bytes=131072-180000",131072,180000,131072,180000);
        interval("bytes=-4",374775,374778,-4,-1);
        interval("bytes=-999999",0,374778,-999999,-1);
        // The same oracle rejects the old pipe: buffered bytes are not total size.
        try {chromium(new Pipe(new byte[243707]),131072,-1);throw new AssertionError();}
        catch(IOException expected) {}
        Pipe source=new Pipe(new byte[]{3,4,5,6,7,8,9});
        OwnerMediaBody body=OwnerMediaBody.adapt(source,"bytes=3-","bytes 3-9/10","7");
        check(body.skip(2)==2);check(body.skip(1)==1);check(body.read()==3);
        check(body.skip(2)==2);check(body.read()==6);body.close();check(source.closed);
        // Without an initial seek the first read retires all virtual credit.
        body=OwnerMediaBody.adapt(new Pipe(new byte[]{3,4,5}),"bytes=3-5","bytes 3-5/10","3");
        check(body.read()==3);check(body.skip(1)==1);check(body.read()==5);check(body.read()==-1);
        source=new Pipe(new byte[]{3});body=OwnerMediaBody.adapt(source,"bytes=3-5","bytes 3-5/10","3");
        try {chromium(body,3,5);throw new AssertionError();} catch(IOException expected) {check(source.closed);}
        for(String length:new String[]{"2","nonsense"}) {
            try {OwnerMediaBody.adapt(new Pipe(new byte[3]),"bytes=3-5","bytes 3-5/10",length);throw new AssertionError();}
            catch(IOException expected) {}
        }
        try {OwnerMediaBody.adapt(new Pipe(new byte[3]),"bytes=4-6","bytes 3-5/10","3");throw new AssertionError();}
        catch(IOException expected) {}
        check(OwnerMediaBody.adapt(new Pipe(new byte[3]),"bytes=3-5,7-9","bytes 3-5/10","3")==null);
        check(OwnerMediaBody.adapt(new Pipe(new byte[3]),"bytes=3-5","bytes 3-5/2147483648","3")==null);
        check(OwnerMediaBody.adapt(new Pipe(new byte[3]),null,null,null)==null);
        // Closing the wrapper must unblock a reader, not wait behind its IO lock.
        CountDownLatch reading=new CountDownLatch(1), closed=new CountDownLatch(1);
        InputStream blocked=new InputStream() {
            public int read() throws IOException {reading.countDown();try {if(!closed.await(3,TimeUnit.SECONDS)) throw new IOException("close blocked");} catch(InterruptedException e){throw new IOException(e);}throw new IOException("closed");}
            public void close(){closed.countDown();}
        };
        OwnerMediaBody waiting=OwnerMediaBody.adapt(blocked,"bytes=0-","bytes 0-9/10","10");
        AtomicReference<Throwable> failure=new AtomicReference<>();
        Thread thread=new Thread(()->{try {waiting.read();failure.set(new AssertionError("read after close"));} catch(IOException expected) {} catch(Throwable unexpected) {failure.set(unexpected);}});
        thread.start();check(reading.await(1,TimeUnit.SECONDS));
        Thread closer=new Thread(()->{try {waiting.close();} catch(Throwable error) {failure.set(error);}});
        closer.start();check(closed.await(1,TimeUnit.SECONDS));thread.join(1000);closer.join(1000);
        check(!thread.isAlive() && !closer.isAlive() && failure.get()==null);
        System.out.println("owner media: Chromium range sequence, exact bytes, genuine skips, truncation and cancellation pass");
    }
}
