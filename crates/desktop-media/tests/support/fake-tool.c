#define _POSIX_C_SOURCE 200809L
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static int exists(const char *name) {
    return access(name, F_OK) == 0;
}

static void payload(const char *root, const char *name, const char *text) {
    char path[8192];
    if (snprintf(path, sizeof(path), "%s/%s", root, name) >= (int)sizeof(path)) exit(92);
    for (char *p = path + 1; *p; ++p) {
        if (*p == '/') {
            *p = 0;
            if (mkdir(path, 0700) != 0 && errno != EEXIST) exit(93);
            *p = '/';
        }
    }
    FILE *file = fopen(path, "wb");
    if (!file) exit(94);
    fputs(text, file);
    if (fclose(file) != 0) exit(95);
}

int main(int argc, char **argv) {
    FILE *args = fopen("native-argv", "ab");
    if (!args) return 90;
    for (int i = 1; i < argc; ++i) {
        fwrite(argv[i], 1, strlen(argv[i]) + 1, args);
    }
    fclose(args);
    FILE *group = fopen("native-pgid", "wb");
    if (!group) return 91;
    fprintf(group, "%ld", (long)getpgrp());
    fclose(group);
    if (exists("delay")) sleep(10);
    if (argc > 1 && (strcmp(argv[1], "-tf") == 0 || strcmp(argv[1], "-tvf") == 0)) return 31;
    int attach = argc > 1 && strcmp(argv[1], "attach") == 0;
    int detach = argc > 1 && strcmp(argv[1], "detach") == 0;
    const char *mount = NULL;
    for (int i = 1; i < argc; ++i) {
        if (strcmp(argv[i], "-mountpoint") == 0 && i + 1 < argc) mount = argv[i + 1];
        if (strcmp(argv[i], "mount") == 0) { attach = 1; mount = argv[argc - 1]; }
        if (strcmp(argv[i], "umount") == 0) detach = 1;
    }
    if (detach) return exists("fail-detach") ? 32 : 0;
    if (attach) {
        if (!mount) return 96;
        payload(mount, "boot/bcd", "fixture bcd");
        payload(mount, "efi/boot/bootaa64.efi", "fixture EFI");
        payload(mount, "efi/microsoft/boot/bcd", "fixture EFI bcd");
        payload(mount, "sources/boot.wim", "fixture boot");
        payload(mount, "sources/install.esd", "fixture install");
        return exists("fail-attach") ? 33 : 0;
    }
    if (exists("fail")) {
        fputs("https://example.invalid/payload?token=secret-never-log\n", stderr);
        return 34;
    }
    fputs("native fixture complete\n", stdout);
    return 0;
}
