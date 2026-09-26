#include <CoreFoundation/CoreFoundation.h>
#include <DiskArbitration/DiskArbitration.h>
#include <ctype.h>
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdbool.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

static volatile sig_atomic_t stopped;
static bool claim_finished;
static bool claimed;

static void stop_requested(int signal_number) {
    (void)signal_number;
    stopped = 1;
}

static void claim_completed(DADiskRef disk, DADissenterRef dissenter, void *context) {
    (void)disk;
    (void)context;
    claimed = dissenter == NULL;
    claim_finished = true;
    if (dissenter != NULL) {
        fprintf(stderr, "Disk claim failed: 0x%x\n", DADissenterGetStatus(dissenter));
    }
}

static DADissenterRef release_requested(DADiskRef disk, void *context) {
    (void)disk;
    (void)context;
    return DADissenterCreate(kCFAllocatorDefault, kDAReturnBusy,
                            CFSTR("WoR-Flasher is preparing this disk."));
}

static bool input_closed(void) {
    char buffer[64];
    ssize_t count = read(STDIN_FILENO, buffer, sizeof(buffer));
    return count == 0 || (count < 0 && errno != EAGAIN && errno != EINTR);
}

static bool whole_disk_path(const char *path) {
    const char *prefix = "/dev/disk";
    if (strncmp(path, prefix, strlen(prefix)) != 0) return false;
    const char *suffix = path + strlen(prefix);
    if (*suffix == '\0') return false;
    for (; *suffix != '\0'; suffix++) {
        if (!isdigit((unsigned char)*suffix)) return false;
    }
    return true;
}

int main(int argc, char **argv) {
    if (argc != 2 || !whole_disk_path(argv[1])) {
        fprintf(stderr, "Usage: macos-disk-claim /dev/diskN < control-pipe\n");
        return 2;
    }
    if (isatty(STDIN_FILENO)) {
        fprintf(stderr, "A control pipe is required; interactive disk claims are refused.\n");
        return 2;
    }
    int input_flags = fcntl(STDIN_FILENO, F_GETFL);
    if (input_flags < 0 || fcntl(STDIN_FILENO, F_SETFL, input_flags | O_NONBLOCK) < 0) {
        perror("control pipe");
        return 1;
    }
    if (input_closed()) return 0;
    signal(SIGINT, stop_requested);
    signal(SIGTERM, stop_requested);
    signal(SIGHUP, stop_requested);
    signal(SIGPIPE, SIG_IGN);

    DASessionRef session = DASessionCreate(kCFAllocatorDefault);
    if (session == NULL) return 1;
    DADiskRef disk = DADiskCreateFromBSDName(kCFAllocatorDefault, session, argv[1]);
    CFDictionaryRef description = disk == NULL ? NULL : DADiskCopyDescription(disk);
    CFTypeRef protocol = description == NULL ? NULL :
        CFDictionaryGetValue(description, kDADiskDescriptionDeviceProtocolKey);
    CFTypeRef model = description == NULL ? NULL :
        CFDictionaryGetValue(description, kDADiskDescriptionDeviceModelKey);
    bool disk_image = protocol != NULL && model != NULL &&
        CFEqual(protocol, CFSTR("Virtual Interface")) && CFEqual(model, CFSTR("Disk Image"));
    bool eligible = description != NULL &&
        CFDictionaryGetValue(description, kDADiskDescriptionMediaWholeKey) == kCFBooleanTrue &&
        (disk_image || CFDictionaryGetValue(description, kDADiskDescriptionDeviceInternalKey) == kCFBooleanFalse) &&
        CFDictionaryGetValue(description, kDADiskDescriptionMediaWritableKey) == kCFBooleanTrue;
    if (description != NULL) CFRelease(description);
    if (!eligible) {
        fprintf(stderr, "Refusing to claim a disk that is not external, writable, and whole.\n");
        if (disk != NULL) CFRelease(disk);
        CFRelease(session);
        return 1;
    }

    DASessionScheduleWithRunLoop(session, CFRunLoopGetCurrent(), kCFRunLoopDefaultMode);
    DADiskClaim(disk, kDADiskClaimOptionDefault, release_requested, NULL, claim_completed, NULL);
    CFAbsoluteTime deadline = CFAbsoluteTimeGetCurrent() + 10;
    while (!stopped && !claim_finished && CFAbsoluteTimeGetCurrent() < deadline) {
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.1, true);
        if (input_closed()) stopped = 1;
    }
    if (claimed && !stopped) {
        if (printf("READY\n") < 0 || fflush(stdout) != 0) stopped = 1;
        while (!stopped) {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.1, true);
            if (input_closed()) stopped = 1;
        }
    }
    if (!claim_finished) fprintf(stderr, "Disk claim canceled or timed out.\n");
    if (claimed) DADiskUnclaim(disk);
    DASessionUnscheduleFromRunLoop(session, CFRunLoopGetCurrent(), kCFRunLoopDefaultMode);
    CFRelease(disk);
    CFRelease(session);
    return claimed ? 0 : 1;
}
