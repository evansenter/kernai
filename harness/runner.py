"""Milestone acceptance runner: `python3 -m harness.runner <milestone>|all`.

Each milestone check is a function returning None on success and raising
AssertionError (with a readable message) on failure. `make test` runs `all`.
"""

import json
import subprocess
import sys
import time

MILESTONES = {}


def milestone(name):
    def register(fn):
        MILESTONES[name] = fn
        return fn
    return register


@milestone("m0")
def m0_framing():
    """Framing layer round-trips frames over a real pipe (loopback stub)."""
    proc = subprocess.run(
        [sys.executable, "-m", "unittest", "discover", "-s", "harness", "-t", "."],
        capture_output=True, text=True)
    if proc.returncode != 0:
        raise AssertionError(f"framing unit tests failed:\n{proc.stderr}")


@milestone("m1")
def m1_boot_hello():
    """Kernel boots; hello frame arrives over the SBI console (acceptance 1)."""
    from .qemu import KERNEL_ELF, QemuKernel
    assert KERNEL_ELF.exists(), f"{KERNEL_ELF} missing — run `make build`"
    with QemuKernel() as q:
        evt = q.next_event(timeout=60)
        assert evt is not None, f"EOF before any frame; stderr: {q.stderr_tail()}"
        assert evt.get("type") == "hello", f"first event is not hello: {evt}"
        assert evt.get("id") == 0, f"hello must be event 0: {evt}"
        assert evt.get("proto") == 0, f"unknown protocol: {evt}"


def _await(q, evt_type, seen, timeout=60):
    """Read events until one of `evt_type` arrives; every event is appended
    to `seen` so callers can assert stream-wide invariants afterwards.
    `timeout` bounds the WHOLE wait — a kernel that keeps ticking but never
    answers must fail the suite, not reset the watchdog per frame."""
    deadline = time.monotonic() + timeout
    while True:
        remaining = deadline - time.monotonic()
        assert remaining > 0, \
            f"no {evt_type} event within {timeout}s (last events: {seen[-3:]})"
        try:
            evt = q.next_event(remaining)
        except TimeoutError as e:
            # A silent guest raises TimeoutError; turn it into a clean failure.
            raise AssertionError(f"no {evt_type} event within {timeout}s: {e}") from e
        assert evt is not None, f"EOF while waiting for {evt_type}; stderr: {q.stderr_tail()}"
        seen.append(evt)
        if evt["type"] == evt_type:
            return evt


@milestone("m2")
def m2_traps_timer_fault():
    """Monotonic timer ticks (acceptance 2); ring query answered; illegal
    instruction yields a structured fault report, then exit (acceptance 3)."""
    import subprocess

    from .qemu import QemuKernel
    with QemuKernel() as q:
        seen = []
        hello = _await(q, "hello", seen)
        assert hello["id"] == 0, f"hello must be event 0: {hello}"

        # Acceptance 2: N timer ticks, monotonically increasing event ids.
        ticks = []
        while len(ticks) < 5:
            ticks.append(_await(q, "tick", seen))
        for a, b in zip(ticks, ticks[1:]):
            assert a["id"] < b["id"], f"tick ids not monotonic: {a} -> {b}"
            assert a["seq"] + 1 == b["seq"], f"tick seq skipped: {a} -> {b}"
            assert a["time"] < b["time"], f"tick time not monotonic: {a} -> {b}"

        # P11 seed: the trap ring answers a serial query.
        q.send(b"r")
        ring = _await(q, "trap_ring", seen)
        assert ring["count"] >= 5, f"ring count below observed ticks: {ring}"
        assert ring["entries"], f"ring dump empty: {ring}"
        assert all(e["cause"] == "timer" for e in ring["entries"]), \
            f"unexpected causes in ring: {ring}"

        # Acceptance 3: deliberate illegal instruction -> structured fault
        # report (cause, sepc, decoded fields), then shutdown — not a hang.
        q.send(b"x")
        fault = _await(q, "fault", seen)
        assert fault["cause"] == "0x2", f"wrong cause: {fault}"
        assert fault["cause_name"] == "illegal_instruction", f"wrong cause_name: {fault}"
        assert int(fault["sepc"], 16) > 0x8020_0000, f"implausible sepc: {fault}"
        assert fault["stval"] == "0xc0001073", f"wrong stval: {fault}"
        insn = fault["insn"]
        assert insn["bits"] == fault["stval"], f"insn bits != stval: {fault}"
        assert insn["opcode"] == "0x73", f"not a SYSTEM opcode: {fault}"
        assert insn["csr"] == "0xc00", f"CSR not decoded: {fault}"
        assert fault["ring"][-1]["cause"] == "illegal_instruction", \
            f"fault missing from its own ring history: {fault}"

        # Stream-wide invariant: event ids strictly increase in emission order.
        ids = [e["id"] for e in seen]
        assert ids == sorted(set(ids)), f"event ids not strictly monotonic: {ids}"

        try:
            q.wait_exit(30)
        except subprocess.TimeoutExpired:
            raise AssertionError("kernel hung after fault report instead of shutting down")


@milestone("m3")
def m3_user_payloads():
    """A U-mode payload runs to sys_exit; its output arrives tagged untrusted;
    a payload fault yields a structured report and kills only the payload —
    the kernel survives and keeps answering."""
    from .qemu import QemuKernel
    with QemuKernel() as q:
        seen = []
        _await(q, "hello", seen)
        q.send(b"p")  # operator triggers the payload suite

        start = _await(q, "payload_start", seen)
        assert start["name"] == "hello", f"first payload not hello: {start}"
        assert start["caps"] == ["write"], f"unexpected caps: {start}"
        # Payloads run in their own address space at a low user VA (M5).
        assert 0 < int(start["entry"], 16) < 0x8000_0000, f"entry not a user VA: {start}"

        out = _await(q, "payload_output", seen)
        assert out["pid"] == start["pid"], f"output pid mismatch: {out}"
        assert out["untrusted"] is True, f"payload output must be tagged untrusted: {out}"
        assert out["data"] == "hello from userspace", f"wrong output: {out}"

        ex = _await(q, "payload_exit", seen)
        assert ex["pid"] == start["pid"] and ex["code"] == 0, f"bad exit: {ex}"

        # Second payload deliberately faults (S-mode CSR write from U-mode).
        cstart = _await(q, "payload_start", seen)
        assert cstart["name"] == "crasher", f"second payload not crasher: {cstart}"
        fault = _await(q, "fault", seen)
        assert fault["origin"] == "payload", f"fault not attributed to payload: {fault}"
        assert fault["pid"] == cstart["pid"], f"fault pid mismatch: {fault}"
        assert fault["cause_name"] == "illegal_instruction", f"wrong cause: {fault}"
        assert fault["insn"]["csr"] == "0x100", f"expected sstatus (0x100) in decode: {fault}"

        done = _await(q, "suite_done", seen)
        assert done["exited"] == 1 and done["faulted"] == 1, f"unexpected suite result: {done}"

        # The kernel must still be alive after a payload crash: it answers.
        q.send(b"r")
        ring = _await(q, "trap_ring", seen)
        assert ring["count"] >= 4, f"kernel unresponsive after payload fault: {ring}"

        ids = [e["id"] for e in seen]
        assert ids == sorted(set(ids)), f"event ids not strictly monotonic: {ids}"


@milestone("m4")
def m4_caps_spawn_deadline():
    """Capability enforcement (ENOCAP + structured denial), spawn with
    attenuation (a child never exceeds its parent — P10), and instruction-count
    deadline kill of a runaway payload (P1/P2). Kernel survives throughout."""
    from .qemu import QemuKernel
    with QemuKernel() as q:
        seen = []
        _await(q, "hello", seen)
        q.send(b"m")
        done = _await(q, "suite_done", seen)

        by_type = {}
        for e in seen:
            by_type.setdefault(e["type"], []).append(e)

        # Capability denial is a structured event, not just an errno.
        denials = by_type.get("syscall_denied", [])
        assert any(d["syscall"] == "write" for d in denials), \
            f"muzzled payload's write was not denied: {denials}"
        assert any(d["syscall"] == "yield" for d in denials), \
            f"spawner's yield (no cap) was not denied: {denials}"

        # The muzzled payload saw the denial and exited 7; it produced no output.
        muzzled = next(e for e in by_type["payload_start"] if e["name"] == "muzzled")
        assert not any(o["pid"] == muzzled["pid"] for o in by_type.get("payload_output", [])), \
            "muzzled payload should have produced no output"
        assert any(x["pid"] == muzzled["pid"] and x["code"] == 7
                   for x in by_type["payload_exit"]), "muzzled did not exit 7"

        # Spawn attenuation (P10): granted ⊆ parent's caps (the lattice never
        # widens), granted ⊆ requested, and specifically yield was stripped
        # because the parent (spawner) lacks it.
        spawn = by_type["payload_spawn"][0]
        assert spawn["attenuated"] is True, f"expected attenuation: {spawn}"
        assert set(spawn["granted"]).issubset(set(spawn["parent_caps"])), \
            f"P10 violated — granted not a subset of parent's caps: {spawn}"
        assert set(spawn["granted"]).issubset(set(spawn["requested"])), \
            f"granted not a subset of requested: {spawn}"
        assert "yield" in spawn["requested"] and "yield" not in spawn["parent_caps"] \
            and "yield" not in spawn["granted"], \
            f"yield should have been attenuated away (parent lacks it): {spawn}"

        # The spawned child actually ran, with only the attenuated caps.
        child = next(e for e in by_type["payload_start"] if e["name"] == "child")
        assert child["caps"] == ["write"], f"child caps not attenuated: {child}"
        assert any(o["pid"] == child["pid"] and "child running" in o["data"]
                   for o in by_type["payload_output"]), "child did not run"

        # The runaway was preempted by its deadline — a kill event, not a hang.
        kills = by_type.get("payload_killed", [])
        assert kills, "runaway payload was not killed by its deadline"
        assert kills[0]["reason"] == "deadline", f"unexpected kill reason: {kills[0]}"
        assert kills[0]["elapsed"] > kills[0]["deadline"], f"kill before deadline: {kills[0]}"

        assert done["killed"] >= 1 and done["exited"] >= 2, f"unexpected suite result: {done}"

        # Liveness: the kernel still answers after all of that.
        q.send(b"r")
        ring = _await(q, "trap_ring", seen)
        assert ring["count"] >= 4, f"kernel unresponsive after M4 suite: {ring}"

        ids = [e["id"] for e in seen]
        assert ids == sorted(set(ids)), f"event ids not strictly monotonic: {ids}"


@milestone("m5")
def m5_memory_isolation():
    """Per-payload paging (Sv39): a payload reading kernel memory faults
    (isolation), a payload writing its own code faults (W^X), each fault
    report carries the page-table walk, and the kernel survives both — a
    clean payload runs afterward in its own address space."""
    from .qemu import QemuKernel
    with QemuKernel() as q:
        seen = []
        _await(q, "hello", seen)
        q.send(b"i")
        done = _await(q, "suite_done", seen)

        by_type = {}
        for e in seen:
            by_type.setdefault(e["type"], []).append(e)
        faults = [e for e in seen if e["type"] == "fault"]
        by_pid = {e["pid"]: e for e in faults}

        # wild (pid 0): U-mode read of kernel memory → load page fault. The
        # page-table walk shows the kernel page is present but U=0.
        wild = by_pid[0]
        assert wild["origin"] == "payload", f"wild fault not attributed: {wild}"
        assert wild["cause_name"] == "load_page_fault", f"wrong cause for wild: {wild}"
        assert int(wild["stval"], 16) >= 0x8000_0000, f"wild didn't reach kernel VA: {wild}"
        assert wild["pagewalk"], f"wild fault missing page-table walk: {wild}"
        assert wild["pagewalk"][-1]["u"] == 0, \
            f"kernel page should be supervisor-only (u=0): {wild['pagewalk']}"

        # wxviol (pid 1): write to own code → store page fault. The walk's leaf
        # shows the text page is executable but not writable (W^X).
        wx = by_pid[1]
        assert wx["cause_name"] == "store_page_fault", f"wrong cause for wxviol: {wx}"
        leaf = wx["pagewalk"][-1]
        assert leaf["x"] == 1 and leaf["w"] == 0, f"W^X not enforced on text page: {leaf}"

        # leaker (pid 2): confused-deputy defense. It handed the kernel a
        # kernel pointer via write(); the kernel refused (EFAULT), so no
        # bytes leaked and it exited 9. It must have produced NO output.
        leaker = next(e for e in by_type["payload_start"] if e["name"] == "leaker")
        assert not any(o["pid"] == leaker["pid"] for o in by_type.get("payload_output", [])), \
            "leaker produced output — the kernel followed a kernel pointer (confused deputy!)"
        assert any(x["pid"] == leaker["pid"] and x["code"] == 9 for x in by_type["payload_exit"]), \
            "leaker's write into kernel memory was not refused"

        # A clean payload ran last and exited — the kernel survived everything.
        assert any(e["type"] == "payload_exit" and e["code"] == 0 for e in seen), \
            "clean payload did not run after the isolation faults"
        assert done["exited"] == 2 and done["faulted"] == 2, f"unexpected suite result: {done}"

        # Liveness after two page faults.
        q.send(b"r")
        ring = _await(q, "trap_ring", seen)
        assert ring["count"] >= 4, f"kernel unresponsive after isolation suite: {ring}"


@milestone("m6")
def m6_checkpoint_fork():
    """Checkpoint/restore/fork (P8): a payload checkpoints itself mid-run; the
    kernel forks that checkpoint into independent continuations that each
    resume from the checkpoint point (not the top), distinguished from the
    original by the snapshot return value."""
    from .qemu import QemuKernel
    with QemuKernel() as q:
        seen = []
        _await(q, "hello", seen)
        q.send(b"f")
        done = _await(q, "suite_done", seen)

        by_type = {}
        for e in seen:
            by_type.setdefault(e["type"], []).append(e)

        # A snapshot was taken.
        assert by_type.get("snapshot"), "no snapshot event"
        starts = by_type["payload_start"]
        outputs = by_type["payload_output"]

        # Exactly one original run (restored:false) and >=2 restored forks.
        originals = [s for s in starts if not s["restored"]]
        restored = [s for s in starts if s["restored"]]
        assert len(originals) == 1, f"expected one original run: {originals}"
        assert len(restored) >= 2, f"expected >=2 forked continuations: {restored}"

        # The pre-checkpoint line was printed exactly ONCE (only the original
        # ran the code before the snapshot); each continuation resumed AFTER
        # it, so it printed the post-snapshot lines but never the pre line.
        pre = [o for o in outputs if o["data"] == "before the checkpoint"]
        assert len(pre) == 1, f"'before the checkpoint' should print once, got {len(pre)}"

        for fork in restored:
            fpid = fork["pid"]
            fout = [o["data"] for o in outputs if o["pid"] == fpid]
            assert "restored continuation (resumed from checkpoint)" in fout, \
                f"fork {fpid} did not resume from the checkpoint: {fout}"
            assert "before the checkpoint" not in fout, \
                f"fork {fpid} re-ran pre-checkpoint code (restore started from the top!): {fout}"
            assert "common tail after the checkpoint" in fout, \
                f"fork {fpid} did not run the post-checkpoint tail: {fout}"

        # Original took the >0 branch; forks took the ==0 branch — the
        # fork()-style distinction makes this a real what-if verb.
        opid = originals[0]["pid"]
        oout = [o["data"] for o in outputs if o["pid"] == opid]
        assert "original branch (kept running)" in oout, f"original didn't take its branch: {oout}"

        assert done["exited"] >= 3, f"expected original + forks to all exit: {done}"

        # Liveness after all the checkpoint machinery.
        q.send(b"r")
        ring = _await(q, "trap_ring", seen)
        assert ring["count"] >= 4, f"kernel unresponsive after checkpoint suite: {ring}"


@milestone("hardening")
def hardening_garbage_input():
    """Garbage serial bytes are ignored; the ring query still answers; 200
    ticks stay contiguous and monotonic under continuous input noise."""
    from .qemu import QemuKernel

    # Anything except the real commands 'r' and 'x'.
    noise = b"\x00\x01\xfe\xffAZ09!@#\n\r\t\xaa\x99qQRX"
    with QemuKernel() as q:
        seen = []
        _await(q, "hello", seen)
        ticks = []
        while len(ticks) < 200:
            q.send(noise)
            ticks.append(_await(q, "tick", seen))
        assert [t["seq"] for t in ticks] == list(range(1, 201)), \
            "ticks lost or reordered under input noise"
        assert all(e["type"] in ("hello", "tick") for e in seen), \
            f"noise triggered an unexpected event: {[e for e in seen if e['type'] not in ('hello', 'tick')]}"
        q.send(b"r")
        ring = _await(q, "trap_ring", seen)
        assert ring["count"] >= 200, f"ring lost traps: {ring}"
        ids = [e["id"] for e in seen]
        assert ids == sorted(set(ids)), "event ids not strictly monotonic under noise"


@milestone("m7")
def m7_deterministic_replay():
    """Deterministic replay of a full operator session (E6). Record a session
    that drives real input (run the payload + checkpoint suites, then crash
    the kernel to shut down) into QEMU's record/replay log; replay it with NO
    live input; the two event streams must be byte-identical — every operator
    input re-fed at the identical instruction count (P9)."""
    import tempfile

    from .qemu import QemuKernel

    def drain(q):
        frames = []
        while True:
            try:
                f = q.stream.next_frame(timeout=60)
            except TimeoutError as e:
                raise AssertionError(f"QEMU did not reach EOF: {e}") from e
            if f is None:
                return frames
            frames.append(f)

    # Drive every payload suite in turn, then crash. Advancing on each
    # `suite_done` feeds operator input at four distinct, widely-separated
    # instruction counts (not just one), so the replay proves QEMU re-injects
    # each recorded byte at its exact recorded instant.
    script = [b"p", b"m", b"i", b"f"]

    with tempfile.NamedTemporaryFile(suffix=".rr") as rr:
        # Record RAW frames (bit comparison, no JSON round-trip). Wait for the
        # boot frame before driving input — a byte sent before the guest is
        # up is dropped in record mode. Send the next suite command on each
        # `suite_done`; after the last suite, crash with 'x' → SBI shutdown →
        # QEMU exits. rr runs throttled to virtual time (no sleep=off); the
        # session is short so this is sub-second.
        with QemuKernel(record=rr.name) as q:
            recorded = []
            hello = q.stream.next_frame(timeout=60)
            assert hello is not None and json.loads(hello)["type"] == "hello", "record: bad boot"
            recorded.append(hello)
            step = 0
            q.send(script[step])
            while True:
                f = q.stream.next_frame(timeout=60)
                if f is None:
                    break
                recorded.append(f)
                if json.loads(f).get("type") == "suite_done":
                    step += 1
                    q.send(script[step] if step < len(script) else b"x")

        # Replay: no live input; QEMU re-feeds the recorded bytes at the same
        # instruction counts.
        with QemuKernel(replay=rr.name) as q:
            replayed = drain(q)

    assert len(recorded) > 20, f"recorded session too short: {len(recorded)} frames"
    # Sanity: the recorded session actually exercised operator input (the 'p'
    # payload suite ran and the 'x' crash shut the kernel down).
    types = {json.loads(f)["type"] for f in recorded}
    assert {"payload_start", "payload_exit", "fault"} <= types, \
        f"recorded session didn't exercise the input script: {sorted(types)}"
    assert recorded == replayed, (
        f"replay diverged: {len(recorded)} recorded vs {len(replayed)} replayed frames; "
        "first mismatch at "
        + next((str(i) for i, (a, b) in enumerate(zip(recorded, replayed)) if a != b), "tail")
    )


@milestone("m8")
def m8_mcp_control_plane():
    """MCP control plane (P4): the kernel is driven purely over JSON-RPC 2.0
    carried inside the same length-prefixed frames. `initialize`, `tools/list`,
    `resources/list`/`read` (incl. the P5 self-describing `spec`), a
    `tools/call run_suite` whose async events stream to `suite_done`,
    client-opId idempotency (a replayed mutating call runs once), and
    structured errors — all without a single command byte. Stream ids stay
    strictly monotonic across control + event frames (the envelope invariant)."""
    from .mcp import Mcp
    from .qemu import QemuKernel

    def drain_suite(q, seen):
        """Read the async event stream a run_suite kicks off, up to suite_done."""
        while True:
            f = q.stream.next_frame(timeout=60)
            assert f is not None, f"EOF before suite_done; stderr: {q.stderr_tail()}"
            evt = json.loads(f)
            seen.append(evt)
            if evt["type"] == "suite_done":
                return evt

    with QemuKernel() as q:
        seen = [json.loads(q.stream.next_frame(timeout=60))]  # boot hello
        assert seen[0]["type"] == "hello"
        m = Mcp(q)

        # MCP handshake.
        init = m.result("initialize", collect=seen)
        assert init["serverInfo"]["name"] == "kernai", f"bad serverInfo: {init}"
        assert init["protocolVersion"], f"no protocolVersion: {init}"
        assert "tools" in init["capabilities"] and "resources" in init["capabilities"]

        # Tools and resources are discoverable (agents discover, not documented).
        tools = {t["name"] for t in m.result("tools/list", collect=seen)["tools"]}
        assert {"run_suite", "crash", "ring_read"} <= tools, f"missing tools: {tools}"
        resources = {r["uri"] for r in m.result("resources/list", collect=seen)["resources"]}
        assert {"trap_ring", "processes", "spec"} <= resources, f"missing resources: {resources}"

        # P5 self-describing surface: the kernel emits its own ABI.
        spec = m.result("resources/read", {"uri": "spec"}, collect=seen)
        sysnames = {s["name"] for s in spec["syscalls"]}
        assert {"exit", "write", "spawn", "snapshot"} <= sysnames, f"spec syscalls: {spec}"
        capnames = {c["name"] for c in spec["caps"]}
        assert {"write", "spawn"} <= capnames, f"spec caps: {spec}"
        assert spec["memory"]["pool_base"] == "0x80400000", f"spec memory: {spec}"

        # tools/call run_suite is async: accepted now, events then stream out.
        acc = m.result("tools/call", {"name": "run_suite", "arguments": {"suite": "p"}},
                       collect=seen)
        assert acc["status"] == "accepted" and acc["suite"] == "p", f"bad accept: {acc}"
        done = drain_suite(q, seen)
        assert done["exited"] == 1 and done["faulted"] == 1, f"suite result: {done}"
        types = {e["type"] for e in seen}
        assert {"payload_start", "payload_output", "payload_exit", "fault"} <= types

        # The process table resource reflects what ran and how it ended.
        procs = {p["name"]: p for p in
                 m.result("resources/read", {"uri": "processes"}, collect=seen)["processes"]}
        assert procs["hello"]["state"] == "exited" and procs["hello"]["exit_code"] == 0
        assert procs["crasher"]["state"] == "faulted", f"crasher state: {procs}"

        # Idempotency (P4): a mutating call carrying a client opId runs exactly
        # once; a replay of the same opId is acknowledged without re-executing.
        m.result("tools/call",
                 {"name": "run_suite", "arguments": {"suite": "i"}, "opId": "isolation-1"},
                 collect=seen)
        drain_suite(q, seen)  # first fire runs the suite
        mark = len(seen)
        dup = m.call("tools/call",
                     {"name": "run_suite", "arguments": {"suite": "i"}, "opId": "isolation-1"},
                     collect=seen)
        assert dup["result"]["status"] == "duplicate", f"replay re-ran: {dup}"
        # Nothing collected during the duplicate call is a payload start — the
        # suite genuinely did not run a second time (only ticks + the response).
        assert not any(e.get("type") == "payload_start" for e in seen[mark:]), \
            "duplicate opId re-executed the suite"

        # Structured errors, kernel stays alive (a bad request is an event).
        assert m.call("bogus/method", collect=seen)["error"]["code"] == -32601
        assert m.call("tools/call", {"name": "nope"}, collect=seen)["error"]["code"] == -32602
        assert m.call("tools/call", {"name": "run_suite", "arguments": {"suite": "zzz"}},
                      collect=seen)["error"]["code"] == -32602

        # Response-safety regressions (from the M8 audit):
        from .framing import encode_frame

        def next_rpc(timeout=15):
            while True:
                fr = q.stream.next_frame(timeout=timeout)
                assert fr is not None, "EOF waiting for rpc frame"
                e = json.loads(fr)
                seen.append(e)
                if e.get("type") == "rpc":
                    return e["rpc"]

        # (a) A huge/control-heavy id must NOT silently drop the response (it
        # used to overflow the frame): the id is clamped and echoed, and the
        # response still arrives.
        big_id = chr(1) * 400
        q.send(encode_frame(('{"id":"' + big_id + '","method":"initialize"}').encode()))
        r = next_rpc()
        assert r["result"]["serverInfo"]["name"] == "kernai", f"big-id lost response: {r}"
        assert isinstance(r["id"], str) and len(r["id"]) <= 64, f"id not clamped: {len(r['id'])}"

        # (b) A malformed numeric id yields valid JSON (id → null), never a
        # broken frame the host can't parse.
        q.send(encode_frame(b'{"id":1.2.3,"method":"initialize"}'))
        r = next_rpc()
        assert r["id"] is None, f"malformed numeric id not nulled: {r['id']!r}"

        # (c) A crafted length with a withheld body must not wedge the command
        # plane. Send a header claiming a 64-byte body but no body; the reader
        # stalls MAX_STALL_WAITS ticks then abandons. Drain well past that
        # window (ticks keep flowing during the stall — itself proof the kernel
        # isn't hung), then a normal request is answered again.
        q.send(b"\xaa\x99\x40\x00\x00\x00")  # magic + len=64, body withheld
        ticks = 0
        while ticks < 25:
            e = json.loads(q.stream.next_frame(timeout=20))
            seen.append(e)
            if e["type"] == "tick":
                ticks += 1
        alive = m.result("initialize", collect=seen, timeout=20)
        assert alive["serverInfo"]["name"] == "kernai", "command plane wedged after withheld frame"

        # The envelope invariant holds across every frame — control and event
        # alike carry a strictly increasing stream id (P12 spine, determinism).
        ids = [e["id"] for e in seen]
        assert ids == sorted(set(ids)), f"stream ids not strictly monotonic: {ids}"

        # The crash tool ends the session: fault → SBI shutdown → EOF.
        m.send("tools/call", {"name": "crash"})
        saw_fault = False
        while True:
            f = q.stream.next_frame(timeout=60)
            if f is None:
                break
            if json.loads(f).get("type") == "fault":
                saw_fault = True
        assert saw_fault, "crash tool did not produce a fault before shutdown"


@milestone("determinism")
def determinism_two_boots():
    """P9 seed (E6): two input-free boots yield byte-identical event streams."""
    from .qemu import QemuKernel

    def capture():
        frames = []
        with QemuKernel() as q:
            while len(frames) < 11:  # hello + 10 ticks
                frame = q.stream.next_frame(timeout=60)
                assert frame is not None, f"EOF during capture; stderr: {q.stderr_tail()}"
                frames.append(frame)
        return frames

    a, b = capture(), capture()
    for i, (fa, fb) in enumerate(zip(a, b)):
        assert fa == fb, f"boot diverged at frame {i}: {fa!r} != {fb!r}"


@milestone("demo")
def demo_runs_clean():
    """The narrated demo (make demo) completes without an assertion firing."""
    try:
        proc = subprocess.run(
            [sys.executable, "-m", "harness.demo", "--fast"],
            capture_output=True, text=True, timeout=300)
    except subprocess.TimeoutExpired as e:
        out = (e.stdout or b"")[-2000:]
        raise AssertionError(f"demo hung (>300s); last output:\n{out}") from e
    if proc.returncode != 0:
        raise AssertionError(f"demo failed:\n{proc.stdout[-2000:]}\n{proc.stderr[-2000:]}")


def main(argv):
    which = argv[1] if len(argv) > 1 else "all"
    names = list(MILESTONES) if which == "all" else [which]
    for name in names:
        check = MILESTONES.get(name)
        if check is None:
            print(f"unknown milestone {name!r}; have: {', '.join(MILESTONES)}")
            return 2
        try:
            check()
        except (AssertionError, KeyError, StopIteration, IndexError, TimeoutError) as e:
            # A missing expected event surfaces as KeyError/StopIteration/
            # IndexError from the lookups; render it as a clean [FAIL], not a
            # traceback. (It still fails — it never lets a regression pass.)
            print(f"[FAIL] {name}: {type(e).__name__}: {e}")
            return 1
        print(f"[ OK ] {name}: {check.__doc__.strip().splitlines()[0]}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
