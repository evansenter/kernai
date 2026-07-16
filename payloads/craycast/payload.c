// A fixed-point raycaster — Wolfenstein-lite, Doom's direct ancestor — written
// in freestanding C, compiled with clang for rv64 (integer-only: the kernel
// runs payloads with sstatus.FS=0, so no FP instruction may be emitted). It
// proves the whole substrate a doomgeneric port needs: a C toolchain into a
// kernai payload, a freestanding libc subset, fixed-point math, a framebuffer
// in .bss, and a blit syscall that hands the frame to the kernel — which reads
// it through the payload's page table (confused-deputy-safe) and emits it as a
// deterministic event. Same input → same frames, byte for byte (P9).

typedef unsigned long usize;
typedef unsigned char u8;
typedef long i64;

// ---- syscall ABI v0 (a7=nr, a0..a2=args, a0=ret) ----
static i64 sys(usize nr, usize a0, usize a1, usize a2) {
    register usize x0 asm("a0") = a0, x1 asm("a1") = a1, x2 asm("a2") = a2, x7 asm("a7") = nr;
    asm volatile("ecall" : "+r"(x0) : "r"(x1), "r"(x2), "r"(x7) : "memory");
    return (i64)x0;
}
#define SYS_EXIT 0
#define SYS_WRITE 1
#define SYS_BLIT 5
static void cwrite(const char* s) {
    usize n = 0;
    while (s[n]) n++;
    sys(SYS_WRITE, (usize)s, n, 0);
}

// ---- freestanding libc subset (clang emits these as libcalls) ----
void* memset(void* d, int c, usize n) {
    u8* p = d;
    while (n--) *p++ = (u8)c;
    return d;
}
void* memcpy(void* d, const void* s, usize n) {
    u8* a = d;
    const u8* b = s;
    while (n--) *a++ = *b++;
    return d;
}

#include "sintab.h"  // static const short SIN[256], fixed x4096; cos = SIN[(a+64)&255]

// ---- fixed-point (16.16) ----
#define FX 16
#define ONE (1 << FX)
static inline i64 fmul(i64 a, i64 b) { return (a * b) >> FX; }

// ---- world ----
#define W 60
#define H 22
#define MAP 16
// 1 = wall, 0 = open. A border ring plus interior structure to make the
// first-person view legibly mazy as the camera turns.
static const char* MAPROWS[MAP] = {
    "################", "#..............#", "#..####..###...#", "#..#.......#...#",
    "#..#..##...#.###", "#.....#....#...#", "#..####....#...#", "#..............#",
    "#....##..####..#", "#....#......#..#", "#.###......##..#", "#.#........#...#",
    "#.#..####..#...#", "#....#..#......#", "#....#..#......#", "################",
};
static u8 fb[W * H];  // the framebuffer, in .bss

static int wall_at(i64 x, i64 y) {
    int cx = (int)(x >> FX), cy = (int)(y >> FX);
    if (cx < 0 || cy < 0 || cx >= MAP || cy >= MAP) return 1;
    return MAPROWS[cy][cx] == '#';
}

// Cast the whole view for a camera at (px,py) facing table-angle `ang`, into fb.
static void render(i64 px, i64 py, int ang) {
    i64 dirx = (i64)SIN[(ang + 64) & 255] << (FX - 12);  // cos, ->16.16
    i64 diry = (i64)SIN[ang & 255] << (FX - 12);         // sin, ->16.16
    // Camera plane, perpendicular to dir, length ~0.66 (a ~66 deg FOV).
    i64 planex = -diry * 66 / 100, planey = dirx * 66 / 100;
    for (int c = 0; c < W; c++) {
        i64 camx = ((2 * c - W) * ONE) / W;  // -1..+1 across the screen
        i64 rdx = dirx + fmul(planex, camx), rdy = diry + fmul(planey, camx);
        // March the ray in ~1/16-cell steps until it meets a wall.
        i64 x = px, y = py;
        int steps = 0, hit = 0;
        for (; steps < 320; steps++) {
            x += rdx >> 4;
            y += rdy >> 4;
            if (wall_at(x, y)) { hit = 1; break; }
        }
        int mid = H / 2, wall = 0, dist = steps ? steps : 1;
        if (hit) {
            int lh = 640 / dist;  // wall height ~ 1/distance (dist is in 1/16-cells)
            if (lh > H) lh = H;
            wall = lh;
        }
        int top = mid - wall / 2, bot = mid + (wall - wall / 2);
        int shade = 230 - dist;  // closer walls are brighter
        if (shade < 40) shade = 40;
        for (int r = 0; r < H; r++) {
            u8 v;
            if (r < top) v = 20;        // ceiling
            else if (r >= bot) v = 70;  // floor
            else v = (u8)shade;         // wall
            fb[r * W + c] = v;
        }
    }
}

void pmain(void) {
    cwrite("raycast: a fixed-point maze, rendered in C, sandboxed by kernai");
    // Stand in an open cell with long sightlines in several directions, and
    // spin a full turn — successive frames differ, and a replay proves they're
    // bit-identical.
    i64 px = (7 * ONE) + ONE / 2, py = (7 * ONE) + ONE / 2;
    for (int f = 0; f < 48; f++) {
        render(px, py, f * 5);  // ~5 table-steps/frame over 48 frames -> ~360 deg
        sys(SYS_BLIT, (usize)fb, W, H);
    }
}
