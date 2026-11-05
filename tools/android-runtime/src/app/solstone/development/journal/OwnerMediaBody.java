// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
package app.solstone.development.journal;

import java.io.IOException;
import java.io.InputStream;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/** Presents an already-selected media interval to WebView's second range seek. */
final class OwnerMediaBody extends InputStream {
    private static final Pattern REQUEST = Pattern.compile("bytes=(\\d*)-(\\d*)");
    private static final Pattern RESPONSE = Pattern.compile("bytes (\\d+)-(\\d+)/(\\d+)");
    private final InputStream source;
    private final int total;
    private long credit, remaining;
    private boolean initial = true;
    private volatile boolean closed;

    private OwnerMediaBody(InputStream source, int total, long start, long length) {
        this.source=source;this.total=total;credit=start;remaining=length;
    }

    // Chromium uses available() as the full representation size, then applies
    // the request Range before reading. A pipe's available bytes are not its size.
    // Unknown and >2 GiB representations cannot express that contract through
    // Java's int-sized available(); leave those existing paths unchanged.
    static OwnerMediaBody adapt(InputStream source, String range, String contentRange,
                                String contentLength) throws IOException {
        if(range==null || contentRange==null) return null;
        Matcher request=REQUEST.matcher(range.trim()), response=RESPONSE.matcher(contentRange.trim());
        if(!request.matches() || !response.matches()) return null;
        try {
            long start=Long.parseLong(response.group(1)), end=Long.parseLong(response.group(2));
            long total=Long.parseLong(response.group(3));
            if(total>Integer.MAX_VALUE) return null;
            if(total<=0 || start>end || end>=total) throw new IOException("invalid media interval");
            long expectedStart, expectedEnd;
            if(request.group(1).isEmpty()) {
                long suffix=Long.parseLong(request.group(2));
                if(suffix<=0) throw new IOException("invalid media suffix");
                expectedStart=Math.max(0,total-suffix);expectedEnd=total-1;
            } else {
                expectedStart=Long.parseLong(request.group(1));
                expectedEnd=request.group(2).isEmpty() ? total-1 : Math.min(total-1,Long.parseLong(request.group(2)));
            }
            long length=end-start+1;
            if(start!=expectedStart || end!=expectedEnd || contentLength!=null && Long.parseLong(contentLength)!=length)
                throw new IOException("media interval changed");
            return new OwnerMediaBody(source,(int)total,start,length);
        } catch(NumberFormatException error) { throw new IOException("invalid media interval",error); }
    }

    public int available() throws IOException {
        if(closed && remaining!=0) throw new IOException("media closed");
        return initial ? total : (int)remaining;
    }

    public long skip(long count) throws IOException {
        if(count<=0) return 0;
        if(closed && remaining!=0) throw new IOException("media closed");
        if(initial && credit>0) {
            long skipped=Math.min(count,credit);credit-=skipped;
            if(credit==0) initial=false;
            return skipped;
        }
        // Subsequent skips really advance the body, including non-seekable pipes.
        byte[] buffer=new byte[(int)Math.min(8192,count)];long skipped=0;
        while(skipped<count) {
            int read=read(buffer,0,(int)Math.min(buffer.length,count-skipped));
            if(read<0) break;
            skipped+=read;
        }
        return skipped;
    }

    public int read() throws IOException {
        byte[] one=new byte[1];int count=read(one,0,1);
        return count<0 ? -1 : one[0]&255;
    }

    public int read(byte[] bytes,int offset,int count) throws IOException {
        if(offset<0 || count<0 || offset>bytes.length-count) throw new IndexOutOfBoundsException();
        if(count==0) return 0;
        initial=false;credit=0;
        if(remaining==0) {close();return -1;}
        if(closed) throw new IOException("media closed");
        try {
            int read=source.read(bytes,offset,(int)Math.min(count,remaining));
            if(read<0) throw new IOException("media ended before its interval");
            remaining-=read;
            if(remaining==0) close();
            return read;
        } catch(IOException error) {
            try {close();} catch(IOException closing) {error.addSuppressed(closing);}
            throw error;
        }
    }

    // Never serialize close behind a blocked read: Stop must close the real pipe.
    public void close() throws IOException {
        if(!closed) {closed=true;source.close();}
    }
}
