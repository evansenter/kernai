// picolibc syscall + stdio backend for the kernai Doom payload. picolibc gives
// us malloc/string/stdio/soft-float; this wires its bottom edge to kernai's
// ecall ABI (a7=nr, a0..a2) and serves the IWAD from a read-only window the
// kernel maps into our address space. Integer-only — the kernel runs us FPU-off.

#include <errno.h>
#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include <sys/stat.h>

#define SYS_EXIT 0
#define SYS_WRITE 1
#define SYS_BLIT 5
#define SYS_FRAME 6

static long ksys(long nr, long a0, long a1, long a2) {
    register long x0 asm("a0") = a0, x1 asm("a1") = a1, x2 asm("a2") = a2, x7 asm("a7") = nr;
    asm volatile("ecall" : "+r"(x0) : "r"(x1), "r"(x2), "r"(x7) : "memory");
    return x0;
}

// ---- the IWAD window (kernel maps it R/U at this fixed VA; QEMU loads the WAD
//      there via -device loader) ----
#define WAD_VA 0x50000000UL
#define WAD_WINDOW (32UL << 20)  // freedoom1 is ~28.8 MiB; window is padded
static const unsigned char* const WAD = (const unsigned char*)WAD_VA;
#define WAD_FD 3
static long wad_pos = 0;

// ---- POSIX layer picolibc's tinystdio calls ----
int write(int fd, const void* buf, size_t len) {
    if (fd == 1 || fd == 2) ksys(SYS_WRITE, (long)buf, (long)len, 0);  // → untrusted output (P7)
    return (int)len;
}
int read(int fd, void* buf, size_t len) {
    if (fd == WAD_FD) {
        long n = (long)len;
        if (wad_pos + n > (long)WAD_WINDOW) n = (long)WAD_WINDOW - wad_pos;
        if (n <= 0) return 0;
        memcpy(buf, WAD + wad_pos, n);
        wad_pos += n;
        return (int)n;
    }
    return 0;
}
int open(const char* path, int flags, ...) {
    (void)flags;
    const char* dot = strrchr(path, '.');
    if (dot && (!strcmp(dot, ".wad") || !strcmp(dot, ".WAD"))) {
        wad_pos = 0;
        return WAD_FD;  // the IWAD, from the mapped window
    }
    return 100;  // discard fd: Doom's config/savegame writes must not fail hard
}
int close(int fd) {
    (void)fd;
    return 0;
}
long lseek(int fd, long off, int whence) {
    if (fd == WAD_FD) {
        if (whence == SEEK_SET) wad_pos = off;
        else if (whence == SEEK_CUR) wad_pos += off;
        else if (whence == SEEK_END) wad_pos = (long)WAD_WINDOW + off;
        return wad_pos;
    }
    return 0;
}
int fstat(int fd, struct stat* st) {
    memset(st, 0, sizeof *st);
    st->st_mode = S_IFREG;
    st->st_size = (fd == WAD_FD) ? (long)WAD_WINDOW : 0;
    return 0;
}
int stat(const char* path, struct stat* st) {
    (void)path;
    memset(st, 0, sizeof *st);
    st->st_mode = S_IFREG;
    st->st_size = (long)WAD_WINDOW;
    return 0;
}
int mkdir(const char* p, unsigned m) {
    (void)p;
    (void)m;
    return 0;
}
int unlink(const char* p) {
    (void)p;
    return 0;
}
int rename(const char* a, const char* b) {
    (void)a;
    (void)b;
    return 0;  // Doom renames its temp savegame into place; we discard writes
}
int isatty(int fd) {
    (void)fd;
    return 1;
}
int access(const char* p, int m) {
    (void)p;
    (void)m;
    return 0;  // every probed path "exists" (Doom hunts for the IWAD)
}

// ---- process / misc ----
static void out_flush(void);  // defined with the stdio streams below
void _exit(int code) {
    out_flush();  // don't lose a trailing partial line
    ksys(SYS_EXIT, code, 0, 0);
    for (;;) {
    }
}
int getpid(void) { return 1; }
int kill(int pid, int sig) {
    (void)pid;
    (void)sig;
    errno = EINVAL;
    return -1;
}
int gettimeofday(void* tv, void* tz) {
    (void)tv;
    (void)tz;
    return 0;
}

// ---- heap: picolibc malloc grows via sbrk over a big .bss arena (the loader
//      zero-fills it). Doom's zone wants ~16 MiB; give slack. ----
#define HEAP_BYTES (24UL << 20)
static char heap[HEAP_BYTES];
static char* brk_ptr = heap;
void* sbrk(intptr_t inc) {
    if (brk_ptr + inc > heap + HEAP_BYTES) {
        errno = ENOMEM;
        return (void*)-1;
    }
    char* old = brk_ptr;
    brk_ptr += inc;
    return old;
}

// ---- stdio streams (picolibc tinystdio): stdout/stderr → serial, stdin unused ----
// Line-buffered: tinystdio calls putc one char at a time, and each write(1,...)
// is a separate ecall the kernel emits as its own `payload_output` event. We
// coalesce a line into one write so Doom's banner is a handful of events, not a
// thousand. Flushed on newline or when the buffer nears the kernel's per-write
// quota (256 B); a run ends after its final newline, so nothing important lingers.
static char out_buf[200];
static unsigned out_len = 0;
static void out_flush(void) {
    if (out_len) {
        write(1, out_buf, out_len);
        out_len = 0;
    }
}
static int uart_putc(char c, FILE* f) {
    (void)f;
    out_buf[out_len++] = c;
    if (c == '\n' || out_len >= sizeof out_buf) {
        out_flush();
    }
    return (unsigned char)c;
}
static int uart_getc(FILE* f) {
    (void)f;
    return -1;
}
static FILE __stdio = FDEV_SETUP_STREAM(uart_putc, uart_getc, NULL, _FDEV_SETUP_RW);
FILE* const stdin = &__stdio;
FILE* const stdout = &__stdio;
FILE* const stderr = &__stdio;

// ---- the blit hook the platform layer calls ----
void kernai_blit(const void* fb, unsigned w, unsigned h) {
    ksys(SYS_BLIT, (long)fb, w, h);
}
// Stream one raw-RGB framebuffer chunk out for the host to reassemble (P7:
// untrusted pixels, base64-framed by the kernel).
void kernai_frame(const void* buf, unsigned len, unsigned seq) {
    ksys(SYS_FRAME, (long)buf, len, seq);
}
