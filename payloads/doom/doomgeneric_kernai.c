// kernai platform layer for doomgeneric: the six DG_* hooks + main(). Doom
// renders into DG_ScreenBuffer (320x200 XRGB); we downscale to a small
// grayscale grid and hand it to the kernel via SYS_BLIT, which emits it as a
// `frame` event. No input is needed: with DG_GetKey returning nothing, Doom
// plays its attract-mode demos itself.
//
// The clock is virtual, not wall time (P9 determinism), fed from two hooks:
//   * DG_DrawFrame advances it ~one 35 Hz tic per rendered frame, so with one
//     frame per doomgeneric_Tick() the game advances ~one tic per frame — the
//     attract demo actually plays instead of crawling.
//   * DG_SleepMs advances it too, because Doom's TryRunTics() spins in a
//     tic-wait loop calling I_Sleep(1) until I_GetTime() moves; on the very
//     first frame (before any DrawFrame) that spin is the *only* thing that can
//     unstick time, so the sleep must advance it or the loop deadlocks.
// Both are deterministic functions of Doom's own execution, which -icount pins,
// so two runs advance the clock identically and a replay is bit-for-bit the same.
#define MS_PER_TIC 29  // 1000/35 rounded — one 35 Hz tic

#include "doomgeneric.h"
#include "doomkeys.h"

#include <stdint.h>
#include <string.h>

void kernai_blit(const void* fb, unsigned w, unsigned h);          // sys_kernai.c
void kernai_frame(const void* buf, unsigned len, unsigned seq);    // sys_kernai.c
int kernai_getkey(void);                                           // sys_kernai.c
long kernai_snapshot(void);                                        // sys_kernai.c

// The checkpoint sentinel symbol (P8 showcase): when the operator sends this
// key, we snapshot the whole game instead of forwarding a Doom key — the
// kernel then forks the checkpoint into independent "what-if" continuations
// that resume from this exact game state and diverge on their own later input.
#define KEY_SNAP_SENTINEL '#'

// Output grid — fits the kernel's blit cap (72x24) and one 2 KiB frame event.
#define OUTW 72
#define OUTH 24
static unsigned char s_frame[OUTW * OUTH];

// Full-color keyframe: every COLOR_EVERY-th frame we also ship the true
// 320x200 screen (RGB) so the host can save a PNG. 1440 B/chunk matches the
// kernel's SYS_FRAME limit (3-aligned → no base64 padding mid-stream).
#ifndef DOOM_COLOR_EVERY
#define DOOM_COLOR_EVERY 24
#endif
#define COLOR_EVERY DOOM_COLOR_EVERY
#define FB_CHUNK 1440
static unsigned char rgb_chunk[FB_CHUNK];
static uint32_t s_frame_no = 0;

// Virtual clock (ms). Advanced by DG_SleepMs — see the header note above.
static uint32_t s_ms = 0;

void DG_Init(void) {}

// Ship the full 320x200 color screen out in FB_CHUNK-sized RGB pieces. The host
// accumulates the chunks and finalizes the image when the next (ASCII) `frame`
// event arrives, so no per-frame dimensions need to cross the wire.
static void dump_color(void) {
    unsigned ci = 0, seq = 0;
    for (int i = 0; i < DOOMGENERIC_RESX * DOOMGENERIC_RESY; i++) {
        uint32_t p = DG_ScreenBuffer[i];
        rgb_chunk[ci++] = (p >> 16) & 0xff;  // R
        rgb_chunk[ci++] = (p >> 8) & 0xff;   // G
        rgb_chunk[ci++] = p & 0xff;          // B
        if (ci == FB_CHUNK) {
            kernai_frame(rgb_chunk, ci, seq++);
            ci = 0;
        }
    }
    if (ci) {
        kernai_frame(rgb_chunk, ci, seq++);
    }
}

void DG_DrawFrame(void) {
    // Full-color keyframe first (the host uses the ASCII frame below as the
    // "color frame complete" delimiter).
    if (s_frame_no % COLOR_EVERY == 0) {
        dump_color();
    }
    for (int oy = 0; oy < OUTH; oy++) {
        int sy = oy * DOOMGENERIC_RESY / OUTH;
        for (int ox = 0; ox < OUTW; ox++) {
            int sx = ox * DOOMGENERIC_RESX / OUTW;
            uint32_t p = DG_ScreenBuffer[sy * DOOMGENERIC_RESX + sx];
            int r = (p >> 16) & 0xff, g = (p >> 8) & 0xff, b = p & 0xff;
            s_frame[oy * OUTW + ox] = (unsigned char)((r * 77 + g * 150 + b * 29) >> 8);
        }
    }
    kernai_blit(s_frame, OUTW, OUTH);
    s_frame_no++;
    s_ms += MS_PER_TIC;  // one tic of virtual time per rendered frame
}

uint32_t DG_GetTicksMs(void) { return s_ms; }
// The virtual clock's only source: Doom's tic-wait spin calls I_Sleep(1), so
// each spin advances 1 ms and the wait escapes after ~one tic — see the header.
void DG_SleepMs(uint32_t ms) { s_ms += ms; }
void DG_SetWindowTitle(const char* t) { (void)t; }

// Operator input (the other half of the agentic loop): the kernel queues key
// bytes the agent sends over serial; we pop them via SYS_GETKEY. Wire format:
// low 7 bits = a symbol from the small protocol below, bit 7 = key release.
// With no input queued, Doom plays its attract-mode demos as before.
static unsigned char sym_to_doomkey(unsigned char sym) {
    switch (sym) {
        case 'u': return KEY_UPARROW;    // forward
        case 'd': return KEY_DOWNARROW;  // back
        case 'l': return KEY_LEFTARROW;  // turn left
        case 'r': return KEY_RIGHTARROW; // turn right
        case 'n': return KEY_ENTER;
        case 'e': return KEY_ESCAPE;
        case 'f': return KEY_FIRE;
        case 's': return KEY_USE;
        case 't': return KEY_TAB;
        default: return sym;  // ASCII passthrough ('y', digits, menu hotkeys)
    }
}

int DG_GetKey(int* pressed, unsigned char* key) {
    for (;;) {
        int v = kernai_getkey();
        if (v < 0) {
            return 0;  // ring empty
        }
        unsigned char sym = (unsigned char)(v & 0x7f);
        if (sym == KEY_SNAP_SENTINEL) {
            if (!(v & 0x80)) {
                kernai_snapshot();  // checkpoint on press; swallow the sentinel
            }
            continue;  // never a Doom key
        }
        *pressed = !(v & 0x80);
        *key = sym_to_doomkey(sym);
        return 1;
    }
}

extern void doomgeneric_Create(int argc, char** argv);
extern void doomgeneric_Tick(void);

#ifndef DOOM_FRAMES
#define DOOM_FRAMES 900  // ~26 s of game time: title screen → demo playback
#endif

int main(void) {
    // Build our own argv (crt0 gives us none): point Doom at the IWAD our file
    // shim serves from the mapped window.
    char* argv[] = {"doom", "-iwad", "freedoom1.wad", 0};
    doomgeneric_Create(3, argv);
    for (int i = 0; i < DOOM_FRAMES; i++) {
        doomgeneric_Tick();
    }
    return 0;
}
