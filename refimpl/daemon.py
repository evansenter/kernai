"""A degraded reference implementation of the kernai surface (`docs/SPEC.md`).

The RFC's evaluation plan calls for "a degraded reference implementation — a
Linux daemon speaking the same surface — no true checkpoint/fork, but real — so
the eval suite becomes a public benchmark for BOTH kernel surfaces and operator
agents." This is that daemon.

It speaks the SAME wire framing (§1) and the SAME MCP control plane (§3) as
kernai — an operator or eval harness drives it identically. What it CANNOT do is
the point: it is an ordinary host process, so its "payloads" are subprocesses,
and a payload fault is a process exit/signal. It has no MMU it controls, so a
fault carries no Sv39 page-table walk; no trap frame, so no register file; no
address space of its own to copy, so no checkpoint/fork. Those absences are not
bugs — they are the degradation, and they are exactly the structured fields
(P6/P11) that the eval measures kernai's surface for. The daemon makes the
thesis falsifiable across implementations: same protocol, different mechanism,
measurably different diagnostic surface.

Run as a module (`python3 -m refimpl.daemon`) to speak the protocol on
stdout/stdin; `harness/conformance.py` drives it and kernai through the same
session and diffs what each surface can answer.
"""

import json
import os
import signal
import struct
import sys

MAGIC = b"\xaa\x99"


class Daemon:
    def __init__(self, out, inp):
        self._out = out
        self._inp = inp
        self._id = 0
        self._procs = []  # {pid,name,state,exit_code}

    # ---- framing (§1): identical wire to kernai ----
    def _emit(self, obj):
        obj["id"] = self._id
        self._id += 1
        body = json.dumps(obj).encode()
        self._out.write(MAGIC + struct.pack("<I", len(body)) + body)
        self._out.flush()

    def _read_frame(self):
        hdr = self._inp.read(6)
        if len(hdr) < 6 or hdr[:2] != MAGIC:
            return None
        (n,) = struct.unpack("<I", hdr[2:6])
        body = self._inp.read(n)
        return json.loads(body)

    def run(self):
        self._emit({"type": "hello", "name": "refimpl", "proto": 0})
        while True:
            req = self._read_frame()
            if req is None:
                return
            self._dispatch(req)

    # ---- MCP control plane (§3) ----
    def _rpc(self, rid, result=None, error=None):
        rpc = {"jsonrpc": "2.0", "id": rid}
        if error is not None:
            rpc["error"] = error
        else:
            rpc["result"] = result
        self._emit({"type": "rpc", "rpc": rpc})

    def _dispatch(self, req):
        rid = req.get("id")
        method = req.get("method")
        params = req.get("params") or {}
        if method == "initialize":
            self._rpc(rid, {
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "refimpl", "version": "0"},
                "capabilities": {"tools": {}, "resources": {}},
            })
        elif method == "tools/list":
            self._rpc(rid, {"tools": [
                {"name": "run_suite", "description": "Run a workload suite (degraded).",
                 "inputSchema": {"type": "object",
                                 "properties": {"suite": {"type": "string"}},
                                 "required": ["suite"]}},
                {"name": "kill", "description": "Kill a live payload by pid.",
                 "inputSchema": {"type": "object",
                                 "properties": {"pid": {"type": "string"}},
                                 "required": ["pid"]}},
            ]})
        elif method == "resources/list":
            self._rpc(rid, {"resources": [
                {"uri": "spec", "name": "spec", "mimeType": "application/json"},
                {"uri": "processes", "name": "processes", "mimeType": "application/json"},
            ]})
        elif method == "resources/read":
            self._read_resource(rid, params.get("uri"))
        elif method == "tools/call":
            self._tools_call(rid, params)
        else:
            self._rpc(rid, error={"code": -32601, "message": "method not found"})

    def _read_resource(self, rid, uri):
        if uri == "spec":
            # P5, honestly degraded: this impl's real ABI is POSIX, not the
            # kernai syscall set, and it exposes no memory map it controls.
            self._rpc(rid, {"proto": 0, "arch": os.uname().machine,
                            "syscalls": [{"name": "posix"}], "caps": [],
                            "memory": None, "degraded": True})
        elif uri == "processes":
            self._rpc(rid, {"processes": self._procs})
        else:
            self._rpc(rid, error={"code": -32602, "message": "unknown resource"})

    def _tools_call(self, rid, params):
        name = params.get("name")
        args = params.get("arguments") or {}
        if name == "run_suite":
            self._rpc(rid, {"status": "accepted", "suite": args.get("suite", "")})
            self._run_suite(args.get("suite", ""))
        elif name == "kill":
            pid = int(args.get("pid", -1))
            slot = next((p for p in self._procs
                         if p["pid"] == pid and p["state"] == "running"), None)
            if slot is None:
                self._rpc(rid, error={"code": -32602, "message": "payload not alive"})
            else:
                slot["state"] = "killed"
                # DEGRADED: no instruction-count clock, so no `elapsed` MTTR fact.
                self._emit({"type": "payload_killed", "pid": pid,
                            "reason": "operator"})
                self._rpc(rid, {"status": "killed", "pid": pid})
        else:
            self._rpc(rid, error={"code": -32602, "message": "unknown tool"})

    # ---- the degradation: payloads are subprocesses; a fault is a signal ----
    def _run_suite(self, suite):
        # A tiny stand-in workload: one clean exit, one "fault" (SIGILL). The
        # eval only needs the fault to compare surfaces.
        work = {
            "e": [("hello", "exit 0"), ("crasher", "kill -ILL $$")],
            "p": [("hello", "exit 0")],
        }.get(suite, [("hello", "exit 0")])
        exited = faulted = 0
        for pid, (nm, sh) in enumerate(work):
            self._procs.append({"pid": pid, "name": nm, "state": "running",
                                "exit_code": 0})
            self._emit({"type": "payload_start", "pid": pid, "name": nm,
                        "caused_by": None})
            code = os.system(sh)
            sig = code & 0x7f
            if sig != 0:
                faulted += 1
                self._procs[pid].update(state="faulted", exit_code=-sig)
                # The whole point, side by side with kernai's fault frame: this
                # is all a host process can say. No scause decode, no faulting
                # instruction, no page-table walk, no register file, no causal
                # DAG — just "it died on signal N".
                self._emit({"type": "fault", "origin": "payload", "pid": pid,
                            "cause_name": signal.Signals(sig).name,
                            "degraded": True,
                            "detail": "process terminated by signal; "
                                      "no trap frame / pagewalk / registers"})
            else:
                exited += 1
                self._procs[pid].update(state="exited",
                                        exit_code=(code >> 8) & 0xff)
                self._emit({"type": "payload_exit", "pid": pid,
                            "code": self._procs[pid]["exit_code"],
                            "caused_by": None})
        self._emit({"type": "suite_done", "exited": exited,
                    "faulted": faulted, "killed": 0})


def main():
    Daemon(sys.stdout.buffer, sys.stdin.buffer).run()


if __name__ == "__main__":
    main()
