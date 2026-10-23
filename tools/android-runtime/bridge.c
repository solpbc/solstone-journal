// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#include <jni.h>
#include <stddef.h>
#include <string.h>
#include <unistd.h>
extern char *journal_run(const char *, const char *, size_t);
extern void journal_free(char *);
extern char *journal_owner_open(const char *, const unsigned char *, size_t, int);
extern void journal_owner_cancel(unsigned long long);
extern char *journal_admit_service_tools(const char *, const char *, const unsigned char *, size_t, const unsigned char *, size_t);

JNIEXPORT jbyteArray JNICALL Java_app_solstone_development_journal_Native_openOwner(JNIEnv *e, jclass c, jstring r, jbyteArray q, jint fd) {
    (void)c;
    const char *root = NULL;
    jbyte *request = NULL;
    char *out = NULL;
    jbyteArray result = NULL;
    if (!r || !q) goto cleanup;
    root = (*e)->GetStringUTFChars(e,r,0);
    if (!root) goto cleanup;
    request = (*e)->GetByteArrayElements(e,q,0);
    if (!request) goto cleanup;
    out = journal_owner_open(root,(unsigned char *)request,(size_t)(*e)->GetArrayLength(e,q),fd);
    fd = -1; /* Rust owns and closes it on every path. */
    result = (*e)->NewByteArray(e,(jsize)strlen(out));
    if (result) (*e)->SetByteArrayRegion(e,result,0,(jsize)strlen(out),(jbyte *)out);
cleanup:
    if (fd >= 0) close(fd);
    if (out) journal_free(out);
    if (request) (*e)->ReleaseByteArrayElements(e,q,request,JNI_ABORT);
    if (root) (*e)->ReleaseStringUTFChars(e,r,root);
    return result;
}

JNIEXPORT void JNICALL Java_app_solstone_development_journal_Native_cancelOwner(JNIEnv *e, jclass c, jlong id) {
    (void)e; (void)c;
    journal_owner_cancel((unsigned long long)id);
}

JNIEXPORT jstring JNICALL Java_app_solstone_development_journal_Native_admitServiceTools(JNIEnv *e, jclass c, jstring n, jstring r, jbyteArray m, jbyteArray b) {
    (void)c;
    if (!n || !r || !m || !b) {
        jclass argument_error=(*e)->FindClass(e,"java/lang/IllegalArgumentException");
        if (argument_error) (*e)->ThrowNew(e,argument_error,"app metadata and signed service assets are required");
        return NULL;
    }
    const char *native=NULL, *resources=NULL;
    jbyte *manifest=NULL, *receipt=NULL;
    char *out=NULL;
    jstring result=NULL;
    native=(*e)->GetStringUTFChars(e,n,0);
    if (!native) goto cleanup;
    resources=(*e)->GetStringUTFChars(e,r,0);
    if (!resources) goto cleanup;
    manifest=(*e)->GetByteArrayElements(e,m,0);
    if (!manifest) goto cleanup;
    receipt=(*e)->GetByteArrayElements(e,b,0);
    if (!receipt) goto cleanup;
    out=journal_admit_service_tools(native,resources,(const unsigned char *)manifest,(size_t)(*e)->GetArrayLength(e,m),(const unsigned char *)receipt,(size_t)(*e)->GetArrayLength(e,b));
    result=(*e)->NewStringUTF(e,out);
cleanup:
    if (out) journal_free(out);
    if (receipt) (*e)->ReleaseByteArrayElements(e,b,receipt,JNI_ABORT);
    if (manifest) (*e)->ReleaseByteArrayElements(e,m,manifest,JNI_ABORT);
    if (resources) (*e)->ReleaseStringUTFChars(e,r,resources);
    if (native) (*e)->ReleaseStringUTFChars(e,n,native);
    return result;
}

JNIEXPORT jstring JNICALL Java_app_solstone_development_journal_Native_run(JNIEnv *e, jclass c, jstring r, jstring o, jint n) {
    (void)c;
    if (!r || !o) {
        jclass argument_error=(*e)->FindClass(e,"java/lang/IllegalArgumentException");
        if (argument_error) (*e)->ThrowNew(e,argument_error,"root and operation are required");
        return NULL;
    }
    const char *root=(*e)->GetStringUTFChars(e,r,0);
    if (!root) return NULL;
    const char *op=(*e)->GetStringUTFChars(e,o,0);
    if (!op) {
        (*e)->ReleaseStringUTFChars(e,r,root);
        return NULL;
    }
    char *out=journal_run(root,op,(size_t)n);
    jstring result=(*e)->NewStringUTF(e,out);
    journal_free(out);
    (*e)->ReleaseStringUTFChars(e,r,root); (*e)->ReleaseStringUTFChars(e,o,op);
    return result;
}
