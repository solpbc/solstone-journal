// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#include <jni.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <limits.h>

typedef int (*saved_copy_fn)(void *,const char *,const char *,const char *);
typedef void (*saved_release_fn)(void *);
extern uint64_t journal_owner_sources_install(const char *,void *,saved_copy_fn,saved_release_fn);
extern void journal_owner_sources_retire(uint64_t);

struct saved_provider {
    JavaVM *vm;
    jweak provider;
    jmethodID copy;
};

static bool saved_environment(JavaVM *vm,JNIEnv **env,bool *attached) {
    *attached=false;
    jint state=(*vm)->GetEnv(vm,(void **)env,JNI_VERSION_1_6);
    if (state == JNI_OK) return true;
    if (state != JNI_EDETACHED) return false;
    if ((*vm)->AttachCurrentThread(vm,(void **)env,NULL) != JNI_OK) return false;
    *attached=true;
    return true;
}

// Standard UTF-8 through String's byte constructor, rather than modified JNI
// UTF-8. A provider URI or destination can contain a supplementary character.
static jstring saved_utf8(JNIEnv *env,const char *text) {
    if (!text) return NULL;
    size_t length=strlen(text);
    if (length > INT_MAX) return NULL;
    jbyteArray bytes=(*env)->NewByteArray(env,(jsize)length);
    if (!bytes) return NULL;
    (*env)->SetByteArrayRegion(env,bytes,0,(jsize)length,(const jbyte *)text);
    if ((*env)->ExceptionCheck(env)) return NULL;
    jclass type=(*env)->FindClass(env,"java/lang/String");
    if (!type) return NULL;
    jmethodID init=(*env)->GetMethodID(env,type,"<init>","([BLjava/lang/String;)V");
    if (!init) return NULL;
    jstring encoding=(*env)->NewStringUTF(env,"UTF-8");
    if (!encoding) return NULL;
    return (jstring)(*env)->NewObject(env,type,init,bytes,encoding);
}

static int saved_provider_copy(void *opaque,const char *identity,const char *material,const char *destination) {
    struct saved_provider *context=(struct saved_provider *)opaque;
    JNIEnv *env=NULL;
    bool attached=false;
    if (!saved_environment(context->vm,&env,&attached)) return 2;
    int result=2;
    if ((*env)->PushLocalFrame(env,24) == JNI_OK) {
        jobject provider=(*env)->NewLocalRef(env,context->provider);
        if (provider) {
            jstring source=saved_utf8(env,identity);
            jstring grant=source ? saved_utf8(env,material) : NULL;
            jstring target=grant ? saved_utf8(env,destination) : NULL;
            if (target && !(*env)->ExceptionCheck(env)) result=(*env)->CallIntMethod(env,provider,context->copy,source,grant,target);
        }
        if ((*env)->ExceptionCheck(env)) { (*env)->ExceptionClear(env);result=2; }
        (*env)->PopLocalFrame(env,NULL);
    } else if ((*env)->ExceptionCheck(env)) (*env)->ExceptionClear(env);
    if (attached) (*context->vm)->DetachCurrentThread(context->vm);
    return result;
}

static void saved_provider_release(void *opaque) {
    struct saved_provider *context=(struct saved_provider *)opaque;
    JNIEnv *env=NULL;
    bool attached=false;
    if (saved_environment(context->vm,&env,&attached)) {
        (*env)->DeleteWeakGlobalRef(env,context->provider);
        if (attached) (*context->vm)->DetachCurrentThread(context->vm);
    }
    free(context);
}

JNIEXPORT jlong JNICALL Java_app_solstone_development_journal_Native_installSavedFolderProvider(
        JNIEnv *env,jclass ignored,jstring cache,jobject provider) {
    (void)ignored;
    if (!cache || !provider) return 0;
    struct saved_provider *context=calloc(1,sizeof(*context));
    if (!context) return 0;
    if ((*env)->GetJavaVM(env,&context->vm) != JNI_OK) { free(context);return 0; }
    jclass type=(*env)->GetObjectClass(env,provider);
    context->copy=type ? (*env)->GetMethodID(env,type,"copySavedFolder","(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)I") : NULL;
    if (type) (*env)->DeleteLocalRef(env,type);
    if (!context->copy) {
        if ((*env)->ExceptionCheck(env)) (*env)->ExceptionClear(env);
        free(context);return 0;
    }
    context->provider=(*env)->NewWeakGlobalRef(env,provider);
    if (!context->provider) { free(context);return 0; }
    // The app-private cache path comes from Service.getCacheDir().
    const char *cache_path=(*env)->GetStringUTFChars(env,cache,NULL);
    if (!cache_path) { saved_provider_release(context);return 0; }
    // Rust consumes context on every return from this valid registration.
    uint64_t identity=journal_owner_sources_install(cache_path,context,saved_provider_copy,saved_provider_release);
    (*env)->ReleaseStringUTFChars(env,cache,cache_path);
    return (jlong)identity;
}

JNIEXPORT void JNICALL Java_app_solstone_development_journal_Native_retireSavedFolderProvider(
        JNIEnv *env,jclass ignored,jlong identity) {
    (void)env;(void)ignored;
    journal_owner_sources_retire((uint64_t)identity);
}
