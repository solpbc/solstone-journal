// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#include <jni.h>
#include <stdbool.h>
#include <stddef.h>
#include <string.h>

extern void journal_free(char *);
extern unsigned long long journal_owner_folder_page_issue(const char *, unsigned long long);
extern unsigned long long journal_owner_folder_page_allocate(void);
extern unsigned long long journal_owner_folder_page_bind(const char *, unsigned long long, unsigned long long);
extern void journal_owner_folder_page_retire(unsigned long long);
extern bool journal_owner_folder_copy_continues(void);
extern unsigned long long journal_owner_folder_page_current(void);
extern char *journal_owner_folder_copy(const char *, const unsigned char *, size_t,
    void *, bool (*)(void *, const char *));

typedef struct {
    JNIEnv *env;
    jobject copier;
    jmethodID copy;
} FolderCallback;

static bool folder_copy_callback(void *opaque, const char *destination) {
    FolderCallback *callback = opaque;
    JNIEnv *e = callback->env;
    jstring path = (*e)->NewStringUTF(e, destination);
    if (!path) return false;
    jboolean result = (*e)->CallBooleanMethod(e, callback->copier, callback->copy, path);
    bool threw = (*e)->ExceptionCheck(e);
    if (threw) (*e)->ExceptionClear(e);
    (*e)->DeleteLocalRef(e, path);
    return !threw && result == JNI_TRUE;
}

JNIEXPORT jbyteArray JNICALL Java_app_solstone_development_journal_Native_copyOwnerFolder(
        JNIEnv *e, jclass ignored, jstring cache, jbyteArray request, jobject copier) {
    (void)ignored;
    const char *cache_path = NULL;
    jbyte *bytes = NULL;
    jclass copier_class = NULL;
    char *out = NULL;
    jbyteArray result = NULL;
    if (!cache || !request || !copier) goto cleanup;
    jsize length = (*e)->GetArrayLength(e, request);
    if (length <= 0 || length > 32*1024) goto cleanup;
    copier_class = (*e)->GetObjectClass(e, copier);
    if (!copier_class) goto cleanup;
    jmethodID method = (*e)->GetMethodID(e, copier_class, "copyTo", "(Ljava/lang/String;)Z");
    if (!method) goto cleanup;
    cache_path = (*e)->GetStringUTFChars(e, cache, NULL);
    if (!cache_path) goto cleanup;
    bytes = (*e)->GetByteArrayElements(e, request, NULL);
    if (!bytes) goto cleanup;
    FolderCallback callback = {e, copier, method};
    out = journal_owner_folder_copy(cache_path, (unsigned char *)bytes, (size_t)length,
        &callback, folder_copy_callback);
    if (!out) goto cleanup;
    result = (*e)->NewByteArray(e, (jsize)strlen(out));
    if (result) (*e)->SetByteArrayRegion(e, result, 0, (jsize)strlen(out), (jbyte *)out);
cleanup:
    if (out) journal_free(out);
    if (bytes) (*e)->ReleaseByteArrayElements(e, request, bytes, JNI_ABORT);
    if (cache_path) (*e)->ReleaseStringUTFChars(e, cache, cache_path);
    if (copier_class) (*e)->DeleteLocalRef(e, copier_class);
    return result;
}

JNIEXPORT jlong JNICALL Java_app_solstone_development_journal_Native_issueFolderPage(
        JNIEnv *e, jclass ignored, jstring root, jlong generation) {
    (void)ignored;
    if (!root) return 0;
    const char *path = (*e)->GetStringUTFChars(e, root, NULL);
    if (!path) return 0;
    unsigned long long page = journal_owner_folder_page_issue(path, (unsigned long long)generation);
    (*e)->ReleaseStringUTFChars(e, root, path);
    return (jlong)page;
}

JNIEXPORT void JNICALL Java_app_solstone_development_journal_Native_retireFolderPage(
        JNIEnv *e, jclass ignored, jlong page) {
    (void)e; (void)ignored;
    journal_owner_folder_page_retire((unsigned long long)page);
}

JNIEXPORT jlong JNICALL Java_app_solstone_development_journal_Native_allocateFolderPage(
        JNIEnv *e, jclass ignored) {
    (void)e; (void)ignored;
    return (jlong)journal_owner_folder_page_allocate();
}

JNIEXPORT jlong JNICALL Java_app_solstone_development_journal_Native_bindFolderPage(
        JNIEnv *e, jclass ignored, jstring root, jlong generation, jlong page) {
    (void)ignored;
    if (!root) return 0;
    const char *path = (*e)->GetStringUTFChars(e,root,NULL);
    if (!path) return 0;
    unsigned long long bound = journal_owner_folder_page_bind(path,(unsigned long long)generation,(unsigned long long)page);
    (*e)->ReleaseStringUTFChars(e,root,path);
    return (jlong)bound;
}

JNIEXPORT jboolean JNICALL Java_app_solstone_development_journal_Native_folderCopyContinues(
        JNIEnv *e, jclass ignored) {
    (void)e; (void)ignored;
    return journal_owner_folder_copy_continues() ? JNI_TRUE : JNI_FALSE;
}

JNIEXPORT jlong JNICALL Java_app_solstone_development_journal_Native_folderPageCurrent(
        JNIEnv *e, jclass ignored) {
    (void)e; (void)ignored;
    return (jlong)journal_owner_folder_page_current();
}
