// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#import <Foundation/Foundation.h>
#include <mach/mach.h>
#include <os/proc.h>
#include <sys/resource.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>
#include <limits.h>
#include <string.h>
#include <sys/stat.h>
#include "Bridge.h"
char *runtime_resources(void) {
    task_vm_info_data_t vm={0}; mach_msg_type_number_t n=TASK_VM_INFO_COUNT;
    kern_return_t rc=task_info(mach_task_self(), TASK_VM_INFO,(task_info_t)&vm,&n);
    struct rusage usage={0}; int urc=getrusage(RUSAGE_SELF,&usage);
    NSDictionary *v=@{@"pid":@(getpid()),@"uid":@(getuid()),@"vm_rc":@(rc),@"rss_bytes":@(vm.resident_size),@"footprint_bytes":@(vm.phys_footprint),@"available_memory_bytes":@(os_proc_available_memory()),@"rusage_rc":@(urc),@"output_blocks":@(usage.ru_oublock),@"input_blocks":@(usage.ru_inblock),@"user_seconds":@(usage.ru_utime.tv_sec+usage.ru_utime.tv_usec/1e6),@"system_seconds":@(usage.ru_stime.tv_sec+usage.ru_stime.tv_usec/1e6)};
    NSData *d=[NSJSONSerialization dataWithJSONObject:v options:0 error:nil];return strndup(d.bytes,d.length);
}
void runtime_resources_free(char *p){free(p);}

char *journal_ios_backup_private_root(void) {
    @autoreleasepool {
        NSFileManager *manager = [NSFileManager defaultManager];
        NSError *error = nil;
        NSURL *support = [manager URLForDirectory:NSApplicationSupportDirectory
            inDomain:NSUserDomainMask appropriateForURL:nil create:YES error:&error];
        if (!support || error) return NULL;
        NSURL *root = [support URLByAppendingPathComponent:@"BackupRuntime" isDirectory:YES];
        const char *path = root.fileSystemRepresentation;
        if (!path || strlen(path) >= PATH_MAX) return NULL;
        // Only this fixed OS-derived startup directory can be created here.
        if (mkdir(path, 0700) != 0 && errno != EEXIST) return NULL;
        int fd = open(path, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
        if (fd < 0) return NULL;
        struct stat held, observed;
        bool valid = fstat(fd, &held) == 0 && S_ISDIR(held.st_mode) &&
            held.st_uid == geteuid() && (held.st_mode & 07777) == 0700;
        if (valid) valid = [root setResourceValue:@YES forKey:NSURLIsExcludedFromBackupKey error:&error];
        if (valid) valid = [manager setAttributes:@{NSFileProtectionKey:NSFileProtectionComplete}
            ofItemAtPath:root.path error:&error];
        NSNumber *excluded = nil;
        [root removeCachedResourceValueForKey:NSURLIsExcludedFromBackupKey];
        if (valid) valid = [root getResourceValue:&excluded forKey:NSURLIsExcludedFromBackupKey error:&error]
            && [excluded isKindOfClass:[NSNumber class]] && excluded.boolValue;
        NSDictionary *attributes = valid ? [manager attributesOfItemAtPath:root.path error:&error] : nil;
        if (valid) valid = [attributes[NSFileProtectionKey] isEqualToString:NSFileProtectionComplete]
            && lstat(path, &observed) == 0 && observed.st_dev == held.st_dev && observed.st_ino == held.st_ino
            && observed.st_uid == held.st_uid && observed.st_mode == held.st_mode;
        int closed = close(fd);
        if (!valid || closed != 0) return NULL;
        return strdup(path);
    }
}
void journal_ios_backup_private_root_free(char *result) { free(result); }

bool journal_ios_qwen_host_process(void) {
    @autoreleasepool {
        return [NSBundle.mainBundle.bundleURL.pathExtension isEqualToString:@"app"];
    }
}

char *journal_ios_qwen_package_root(void) {
    @autoreleasepool {
        NSURL *root = NSBundle.mainBundle.bundleURL;
        if ([root.pathExtension isEqualToString:@"appex"]) {
            if (![root.lastPathComponent isEqualToString:@"JournalWorker.appex"]) return NULL;
            NSURL *extensions = root.URLByDeletingLastPathComponent;
            if (![extensions.lastPathComponent isEqualToString:@"Extensions"]) return NULL;
            root = extensions.URLByDeletingLastPathComponent;
        }
        if (![root.pathExtension isEqualToString:@"app"]) return NULL;
        const char *value = root.fileSystemRepresentation;
        if (!value || strlen(value) >= PATH_MAX) return NULL;
        return strdup(value);
    }
}
void journal_ios_qwen_package_root_free(char *result) { free(result); }
