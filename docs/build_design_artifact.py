"""Regenerate docs/design.html — the shareable visual companion to DESIGN.md.

Embeds DOOM screenshots (from harness/doom_frames/, produced by `make doom` /
`make doom-play` / `make doom-fork`) as data URIs into a self-contained,
theme-aware HTML page. Run: python3 docs/build_design_artifact.py
"""
import base64
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
FR = ROOT / "harness" / "doom_frames"
OUT = ROOT / "docs" / "design.html"


def uri(name):
    data = (FR / name).read_bytes()
    return "data:image/png;base64," + base64.b64encode(data).decode()


IMG = {k: uri(v) for k, v in {
    "title": "doom_frame_000.png",
    "play": "play_014.png",
    "base": "fork_base.png",
    "left": "fork_A_left.png",
    "right": "fork_B_right.png",
}.items()}

# Per-fault E1 scores (agentic facts, classic facts, total) — from harness/eval.py.
E1 = [
    ("crasher", "illegal instruction · decoded CSR", 5, 2, 5),
    ("wild", "load fault · supervisor page (u=0)", 4, 2, 4),
    ("nullread", "load fault · null deref (v=0)", 4, 2, 4),
    ("wxviol", "store fault · W^X (x=1,w=0)", 4, 2, 4),
    ("stackover", "store fault · stack exhausted", 5, 2, 5),
    ("badjump", "fetch fault · nothing mapped (v=0)", 4, 2, 4),
    ("execdata", "fetch fault · data page (v=1,x=0)", 5, 2, 5),
    ("breaker", "breakpoint · trap instruction", 4, 2, 4),
    ("misalign", "misaligned atomic access", 5, 3, 5),
]

PRINCIPLES = [
    ("P1", "Policy externalized", "Kill / spare / budget are MCP tools; the kernel never decides them. A refused syscall, a parked payload, a preemption — each an event, not a hardwired reaction."),
    ("P2", "Everything bounded", "Instruction-count deadlines under -icount; a runaway becomes a structured payload_killed with elapsed. Per-write byte quota bounds output."),
    ("P3", "Attention is metered", "The digest resource takes a budget and coalesces the tick firehose; the autonomy dial suppresses trace events — and, on DOOM builds, the colour-frame stream."),
    ("P4", "Structured control plane", "MCP / JSON-RPC over the serial frames; typed tools and resources; opId idempotency because agents retry."),
    ("P5", "Self-describing surface", "The spec resource emits the syscall table, capability lattice, and memory map — the ABI as data the agent discovers, not docs that drift."),
    ("P6", "Diagnosis without a debugger", "The fault frame decodes scause, the offending instruction, the Sv39 page-table walk with permission bits, all 31 registers, and the causal parent."),
    ("P7", "Provenance", "Payload output is tagged untrusted, confined to a JSON string — it can never reach the operator as a kernel directive. E5 proves the tag is load-bearing."),
    ("P8", "Checkpoint / fork", "sys_snapshot deep-copies a payload's address space + frame + CapSet; restore/fork build independent continuations. Demonstrated on a live DOOM game."),
    ("P9", "Determinism", "-icount makes execution a function of instruction flow; two input-free boots are byte-identical. Recorded operator input replays bit-for-bit."),
    ("P10", "Attenuation", "A spawned child's CapSet is requested & parent & ceiling — monotonically shrinking. E7 fuzzes it: every spawn event asserted against the lattice."),
    ("P11", "Total introspectability", "Page tables walk themselves into JSON; the scheduler emits why it preempted; the trap ring, process table, and allocator state are all resources."),
    ("P12", "Causal spine", "Every event carries a monotonic stream id and names its caused_by start event — the log is a DAG rooted at each payload's birth, not a flat line."),
]

EVALS = [
    ("E1", "Diagnostic sufficiency", "40 / 40 vs 19 / 40 localization facts", "The structured surface exposes every root-cause fact across 9 seeded faults; the classic printf twin, 19. The gap is exactly the debugger-only detail P6 restores.", "headline"),
    ("E2", "Live-incident MTTR", "remediated mid-run, two ways", "A deadline-less livelock killed while running via the kill tool (through an M13 preemption), or ended by tightening its budget with set_budget."),
    ("E4", "Operator ablation", "live ≫ static ≈ random", "The same incident under three policies. The live operator mitigates all; static defaults and a random policy, none. The gap is what judgement is worth."),
    ("E5", "Injection red-team", "grant rate flips on P7 alone", "A payload emits a fake directive. A fixed operator policy refuses it framed, obeys it stripped — the improper-grant rate flips on provenance framing alone."),
    ("E6", "Replay fidelity", "byte-identical", "Two input-free boots produce identical event streams; a recorded operator session replays bit-for-bit under QEMU record/replay. Both in CI."),
    ("E7", "Attenuation soundness", "no widening · no leak", "A fuzzer sweeps requested caps; every spawn is asserted granted ⊆ parent. The allocator returns to baseline after every suite — no leak across any reap path."),
    ("E8", "Token economics", "governed ≪ firehose", "Operator tokens ≈ wire bytes: reactive vs autonomous vs the budgeted digest, quantified. Suppressed ticks are still accounted, never lost."),
]


def e1_rows():
    out = []
    for name, desc, a, c, t in E1:
        ap = round(100 * a / t)
        cp = round(100 * c / t)
        out.append(f'''<tr>
  <th scope="row"><span class="fault-name">{name}</span><span class="fault-desc">{desc}</span></th>
  <td class="bar-cell">
    <div class="bar"><div class="bar-fill agentic" style="width:{ap}%"></div><span class="bar-num">{a}</span></div>
    <div class="bar"><div class="bar-fill classic" style="width:{cp}%"></div><span class="bar-num">{c}</span></div>
  </td>
</tr>''')
    return "\n".join(out)


def principle_cards():
    out = []
    for code, title, body in PRINCIPLES:
        out.append(f'''<article class="principle">
  <span class="pcode">{code}</span>
  <h3>{title}</h3>
  <p>{body}</p>
</article>''')
    return "\n".join(out)


def eval_rows():
    out = []
    for e in EVALS:
        code, title, metric, body = e[0], e[1], e[2], e[3]
        head = " head" if len(e) > 4 else ""
        out.append(f'''<div class="eval{head}">
  <div class="eval-code">{code}</div>
  <div class="eval-body">
    <div class="eval-top"><h3>{title}</h3><span class="metric">{metric}</span></div>
    <p>{body}</p>
  </div>
</div>''')
    return "\n".join(out)


HTML = f'''<article class="doc">

<header class="hero">
  <div class="hero-grid">
    <div class="hero-lede">
      <p class="eyebrow">Design document · agent-native kernel</p>
      <h1>kernai</h1>
      <p class="tagline">The operating system as a <em>structured surface an agent operates</em> — not a black box a human debugs with gdb.</p>
      <ul class="stat-row">
        <li><b>riscv64</b><span>no_std Rust</span></li>
        <li><b>55 / 200</b><span>unsafe lines</span></li>
        <li><b>22</b><span>CI checks</span></li>
        <li><b>P1–P12</b><span>each tested</span></li>
        <li><b>runs</b><span>DOOM</span></li>
      </ul>
    </div>
    <figure class="term" aria-label="An example fault event on the wire">
      <div class="term-bar"><span class="dot"></span><span class="dot"></span><span class="dot"></span><span class="term-title">fault · wire event</span></div>
      <pre><code><span class="k">{{</span>
  <span class="key">"type"</span>: <span class="str">"fault"</span>, <span class="key">"pid"</span>: <span class="num">1</span>, <span class="key">"caused_by"</span>: <span class="num">42</span>,
  <span class="key">"cause_name"</span>: <span class="str">"store_page_fault"</span>,
  <span class="key">"sepc"</span>: <span class="str">"0x1a4c"</span>, <span class="key">"stval"</span>: <span class="str">"0xf28"</span>,
  <span class="key">"insn"</span>: <span class="k">null</span>,
  <span class="key">"pagewalk"</span>: <span class="k">[</span>
    <span class="k">{{</span> <span class="key">"level"</span>:<span class="num">0</span>, <span class="key">"v"</span>:<span class="num">1</span>, <span class="key">"r"</span>:<span class="num">1</span>, <span class="key">"w"</span>:<span class="num">0</span>, <span class="key">"x"</span>:<span class="num">1</span> <span class="k">}}</span>  <span class="cmt">// W^X denied</span>
  <span class="k">]</span>,
  <span class="key">"regs"</span>: <span class="k">{{</span> <span class="key">"sp"</span>:<span class="str">"0xf40"</span>, <span class="cmt">/* …31 GPRs… */</span> <span class="k">}}</span>,
  <span class="key">"ring"</span>: <span class="k">[</span> <span class="cmt">/* last 8 traps */</span> <span class="k">]</span>
<span class="k">}}</span><span class="cursor"></span></code></pre>
    </figure>
  </div>
</header>

<section class="prose">
  <p class="drop">A conventional kernel externalizes almost nothing. Its state lives in structures you see only through a debugger; a fault is a printf line or a panic; policy — what to schedule, what to kill, what to grant — is hardwired in C. That is right when a human is the operator. It is <em>wrong</em> when the operator is a language model: worse than a human at reading a hex dump, far better at consuming structured, self-describing data and acting through a typed interface.</p>
  <p>kernai inverts the defaults. Mechanism stays in the kernel, dumb and autonomous; every <em>policy</em> decision is externalized to a host-side agent over a structured control plane. A page fault is a JSON event with the faulting instruction decoded and the page-table walk rendered. A runaway is a parked process the operator inspects and kills live. The kernel describes its own ABI as a resource. And the same inputs replay bit-for-bit. The whole thing is small, safe Rust — and to prove the isolation model holds for a real workload rather than toys, it runs DOOM.</p>
</section>

<section>
  <div class="section-head"><span class="marker">§</span><h2>The twelve principles, and the mechanism behind each</h2></div>
  <div class="principles">
    {principle_cards()}
  </div>
</section>

<section class="scorecard">
  <div class="section-head"><span class="marker">E1</span><h2>Diagnostic sufficiency — the headline experiment</h2></div>
  <p class="section-intro">Mechanism held constant, the interface varied: the same kernel renders every fault two ways. Nine seeded faults, each needing a <em>different</em> root cause — two load faults separated only by a page-table walk, two store faults separated by <code>sp</code> and a permission bit. The structured <span class="tag agentic">agentic</span> surface exposes every localization fact; the <span class="tag classic">classic</span> printf twin drops the root-cause detail a log line can't carry.</p>
  <div class="score-wrap">
    <table class="scores">
      <thead><tr><th scope="col">seeded fault</th><th scope="col"><span class="tag agentic">agentic</span> vs <span class="tag classic">classic</span> · facts recovered</th></tr></thead>
      <tbody>
        {e1_rows()}
      </tbody>
      <tfoot><tr><th scope="row">total</th><td class="total"><b class="agentic-t">40 / 40</b> &nbsp;·&nbsp; <b class="classic-t">19 / 40</b></td></tr></tfoot>
    </table>
  </div>
  <p class="section-note">The gap <em>is</em> the thesis: the surface, not the agent, decides debuggability. Run <code>make eval</code> for the live scorecard; <code>make agent-eval</code> puts an operator in front of both and measures the localization rate directly.</p>
</section>

<section>
  <div class="section-head"><span class="marker">E2–E8</span><h2>The rest of the evaluation spine</h2></div>
  <div class="evals">
    {eval_rows()}
  </div>
  <p class="section-note">All eight run in CI (deterministic, stdlib-only) except the model-gated measurements, which swap a real operator in via <code>make agent-eval&nbsp;KERNAI_OPERATOR=llm</code>. A degraded reference implementation speaks the same surface as a plain host process — <code>make conformance</code> shows kernai's fault frame carries six structured fields the degraded backend structurally cannot.</p>
</section>

<section class="capstone">
  <div class="section-head"><span class="marker">▶</span><h2>DOOM — the capstone workload</h2></div>
  <p class="section-intro">The isolation model needed a stress test bigger than a hand-written fixture. Full doomgeneric DOOM — ~80 C translation units, linked against picolibc, playing the freely-licensed Freedoom IWAD — runs as an ordinary sandboxed U-mode payload: memory-isolated, FPU-off (it's fixed-point, which is <em>why</em> it ports), deterministic under -icount. Behind an off-by-default feature, so <code>make test</code> stays pure Rust.</p>

  <div class="shots two">
    <figure><img src="{IMG['title']}" alt="Freedoom title screen rendered by kernai"><figcaption>Boots, maps the IWAD from a read-only window, renders the title.</figcaption></figure>
    <figure><img src="{IMG['play']}" alt="DOOM first-person gameplay, pistol firing"><figcaption>An agent plays E1M1 over MCP — keys in via <code>SYS_GETKEY</code>, pixels out via <code>SYS_FRAME</code>.</figcaption></figure>
  </div>

  <div class="fork">
    <p class="fork-lede"><span class="pcode inline">P8</span> <b>One saved instant, two futures.</b> DOOM checkpoints itself mid-level; the kernel forks the snapshot into two continuations that resume from the <em>identical</em> game state (same HUD, same position) and diverge on their own later input. This is what forced the one subtle kernel change the port needed — <code>deep_copy</code> aliases the read-only IWAD window instead of copying it, so snapshot/fork composes with windowed payloads.</p>
    <div class="shots three">
      <figure><img src="{IMG['base']}" alt="The checkpoint game state"><figcaption>the checkpoint</figcaption></figure>
      <figure><img src="{IMG['left']}" alt="Continuation A turned left"><figcaption>future A · turned left</figcaption></figure>
      <figure><img src="{IMG['right']}" alt="Continuation B turned right"><figcaption>future B · turned right</figcaption></figure>
    </div>
  </div>
  <p class="section-note">The whole DOOM surface added four feature-gated primitives — a read-only device window, a colour framebuffer-out syscall, an operator key ring, the deep-copy window alias — and <em>no change to the default kernel ABI or any acceptance check</em>. <code>make doom</code> boots it; <code>make doom-play</code> runs the agent policy; <code>make doom-fork</code> runs the fork above.</p>
</section>

<footer class="foot">
  <div class="foot-grid">
    <div>
      <p class="eyebrow">Deliberately not here</p>
      <p>No SMP, networking, filesystem, x86, or POSIX — hard non-goals. Open edges are measurement and reach, not mechanism: the model-gated eval runs across several models, a general time-sliced scheduler, keyed-DOOM replay at scale, sound.</p>
    </div>
    <div>
      <p class="eyebrow">Working invariants</p>
      <p><code>make test</code> is the gate — 22 checks from a fresh clone. <code>unsafe</code> only under <code>hal/</code>, ≤4 files, ≤200 lines, every block with a <code>// SAFETY:</code>. Three live docs; determinism routed through one QEMU entry point so it can't drift.</p>
    </div>
  </div>
  <p class="sig">kernai · a research kernel arguing one thing — <b>the interface, not the model, decides how well an agent can operate a system</b> — with mechanisms you can run, evals you can score, and a game you can watch.</p>
</footer>

</article>
'''

STYLE = r"""<style>
:root{
  --ground:#f4f1ea; --panel:#fbfaf6; --panel-2:#efe9dd; --line:#ddd4c4;
  --ink:#23262b; --ink-dim:#4c545e; --muted:#6c7580;
  --accent:#a8620f; --accent-soft:rgba(168,98,15,.11); --accent-line:rgba(168,98,15,.32);
  --cool:#2a7a6f; --cool-soft:rgba(42,122,111,.12);
  --good:#4c8c40; --bad:#bf453e;
  --term-bg:#171b21; --term-ink:#e9e6dd; --term-line:#2c333d;
  --mono:ui-monospace,"SF Mono","JetBrains Mono","Cascadia Code",Menlo,Consolas,monospace;
  --sans:ui-sans-serif,system-ui,-apple-system,"Segoe UI",Roboto,Helvetica,Arial,sans-serif;
  --measure:66ch; --r:14px;
}
@media (prefers-color-scheme:dark){
  :root:not([data-theme="light"]){
    --ground:#0f1217; --panel:#161b22; --panel-2:#1c222b; --line:#2a323d;
    --ink:#e9e7e0; --ink-dim:#bcc3cc; --muted:#8a95a1;
    --accent:#e4a54d; --accent-soft:rgba(228,165,77,.13); --accent-line:rgba(228,165,77,.30);
    --cool:#5bbaac; --cool-soft:rgba(91,186,172,.14);
    --good:#83c170; --bad:#e2665e;
    --term-bg:#0b0e12; --term-ink:#e9e6dd; --term-line:#232a33;
  }
}
:root[data-theme="dark"]{
  --ground:#0f1217; --panel:#161b22; --panel-2:#1c222b; --line:#2a323d;
  --ink:#e9e7e0; --ink-dim:#bcc3cc; --muted:#8a95a1;
  --accent:#e4a54d; --accent-soft:rgba(228,165,77,.13); --accent-line:rgba(228,165,77,.30);
  --cool:#5bbaac; --cool-soft:rgba(91,186,172,.14);
  --good:#83c170; --bad:#e2665e;
  --term-bg:#0b0e12; --term-ink:#e9e6dd; --term-line:#232a33;
}

*{box-sizing:border-box}
body{
  margin:0; background:var(--ground); color:var(--ink);
  font-family:var(--sans); font-size:17px; line-height:1.62;
  -webkit-font-smoothing:antialiased; text-rendering:optimizeLegibility;
}
.doc{max-width:1120px; margin:0 auto; padding:clamp(20px,5vw,64px) clamp(18px,5vw,56px) 96px}
h1,h2,h3{text-wrap:balance; line-height:1.12; margin:0}
p{margin:0}
code{font-family:var(--mono); font-size:.9em; background:var(--accent-soft);
  color:var(--accent); padding:.08em .38em; border-radius:5px}
em{font-style:normal; color:var(--accent); font-weight:600}
b,strong{font-weight:700}
.eyebrow{font-family:var(--mono); font-size:.7rem; letter-spacing:.22em;
  text-transform:uppercase; color:var(--muted); margin:0}

/* ---- hero ---- */
.hero{padding:8px 0 40px; border-bottom:1px solid var(--line); margin-bottom:52px}
.hero-grid{display:grid; grid-template-columns:1.05fr 1fr; gap:clamp(24px,4vw,56px); align-items:center}
.hero-lede{animation:rise .7s cubic-bezier(.2,.7,.2,1) both}
h1{font-family:var(--mono); font-size:clamp(3.2rem,9vw,5.2rem); font-weight:700;
  letter-spacing:-.03em; margin:.18em 0 .28em; color:var(--ink)}
h1::after{content:"_"; color:var(--accent); animation:blink 1.2s steps(1) infinite}
.tagline{font-size:clamp(1.12rem,2vw,1.4rem); line-height:1.4; color:var(--ink-dim); max-width:32ch}
.stat-row{list-style:none; display:flex; flex-wrap:wrap; gap:10px; padding:0; margin:28px 0 0}
.stat-row li{display:flex; flex-direction:column; gap:2px; padding:9px 15px;
  background:var(--panel); border:1px solid var(--line); border-radius:10px}
.stat-row b{font-family:var(--mono); font-size:1.02rem; color:var(--accent); letter-spacing:-.01em}
.stat-row span{font-size:.68rem; letter-spacing:.13em; text-transform:uppercase; color:var(--muted)}

.term{margin:0; background:var(--term-bg); border:1px solid var(--term-line);
  border-radius:var(--r); overflow:hidden; box-shadow:0 24px 60px -30px rgba(0,0,0,.55);
  animation:rise .7s .1s cubic-bezier(.2,.7,.2,1) both}
.term-bar{display:flex; align-items:center; gap:7px; padding:11px 15px;
  background:rgba(255,255,255,.03); border-bottom:1px solid var(--term-line)}
.dot{width:11px; height:11px; border-radius:50%; background:#3a434e}
.dot:nth-child(1){background:#e0655d}.dot:nth-child(2){background:#e4a54d}.dot:nth-child(3){background:#83c170}
.term-title{margin-left:auto; font-family:var(--mono); font-size:.7rem;
  letter-spacing:.14em; text-transform:uppercase; color:#7d8794}
.term pre{margin:0; padding:20px 22px; overflow-x:auto}
.term code{font-family:var(--mono); font-size:.82rem; line-height:1.75;
  background:none; color:var(--term-ink); padding:0; border-radius:0}
.term .k{color:#8893a0}.term .key{color:#7fc7ea}.term .str{color:#e4a54d}
.term .num{color:#83c170}.term .cmt{color:#5f6b78; font-style:italic}
.cursor{display:inline-block; width:9px; height:1.05em; vertical-align:-.15em;
  margin-left:3px; background:var(--accent); animation:blink 1.2s steps(1) infinite}

/* ---- prose ---- */
.prose{max-width:var(--measure); margin:0 auto 72px; display:flex; flex-direction:column; gap:20px}
.prose p{font-size:1.06rem; color:var(--ink-dim)}
.prose .drop::first-letter{font-family:var(--mono); font-weight:700; float:left;
  font-size:3.3rem; line-height:.82; padding:.05em .12em 0 0; color:var(--accent)}

/* ---- section scaffold ---- */
section{margin:0 0 72px}
.section-head{display:flex; align-items:baseline; gap:16px; margin:0 0 28px;
  padding-bottom:16px; border-bottom:1px solid var(--line)}
.section-head h2{font-size:clamp(1.4rem,2.6vw,1.95rem); font-weight:680; letter-spacing:-.015em}
.marker{font-family:var(--mono); font-size:.82rem; font-weight:700; letter-spacing:.06em;
  color:var(--accent); background:var(--accent-soft); border:1px solid var(--accent-line);
  padding:5px 10px; border-radius:8px; flex:none; align-self:center}
.section-intro{max-width:var(--measure); color:var(--ink-dim); margin:0 0 26px; font-size:1.04rem}
.section-note{max-width:var(--measure); color:var(--muted); font-size:.92rem; margin:26px 0 0}

/* ---- principles ---- */
.principles{display:grid; grid-template-columns:repeat(auto-fill,minmax(272px,1fr)); gap:14px}
.principle{position:relative; background:var(--panel); border:1px solid var(--line);
  border-radius:var(--r); padding:20px 20px 22px; transition:border-color .2s,transform .2s}
.principle:hover{border-color:var(--accent-line); transform:translateY(-2px)}
.pcode{font-family:var(--mono); font-size:.72rem; font-weight:700; letter-spacing:.08em;
  color:var(--accent)}
.pcode.inline{background:var(--accent-soft); border:1px solid var(--accent-line);
  padding:2px 8px; border-radius:6px; margin-right:6px}
.principle h3{font-size:1.06rem; font-weight:660; margin:.35em 0 .5em; letter-spacing:-.01em}
.principle p{font-size:.92rem; color:var(--ink-dim); line-height:1.55}

/* ---- E1 scorecard ---- */
.score-wrap{overflow-x:auto; border:1px solid var(--line); border-radius:var(--r); background:var(--panel)}
table.scores{width:100%; border-collapse:collapse; min-width:520px}
.scores th,.scores td{text-align:left; padding:13px 20px; border-bottom:1px solid var(--line)}
.scores thead th{font-family:var(--mono); font-size:.68rem; letter-spacing:.13em;
  text-transform:uppercase; color:var(--muted); font-weight:600}
.scores tbody th{font-weight:500}
.fault-name{font-family:var(--mono); font-size:.9rem; color:var(--ink); display:block}
.fault-desc{font-size:.78rem; color:var(--muted); display:block; margin-top:2px}
.bar-cell{display:flex; flex-direction:column; gap:7px; min-width:240px}
.bar{position:relative; height:15px; background:var(--panel-2); border-radius:5px; overflow:hidden}
.bar-fill{position:absolute; inset:0 auto 0 0; border-radius:5px;
  animation:grow 1s cubic-bezier(.2,.7,.2,1) both}
.bar-fill.agentic{background:linear-gradient(90deg,var(--accent),var(--accent))}
.bar-fill.classic{background:var(--muted); opacity:.55}
.bar-num{position:absolute; right:8px; top:50%; transform:translateY(-50%);
  font-family:var(--mono); font-size:.68rem; font-variant-numeric:tabular-nums;
  color:var(--ink); mix-blend-mode:normal}
.scores tfoot th,.scores tfoot td{border-bottom:none; padding-top:16px; padding-bottom:16px}
.total{font-family:var(--mono); font-variant-numeric:tabular-nums; font-size:1.05rem}
.agentic-t{color:var(--accent)}.classic-t{color:var(--muted)}
.tag{font-family:var(--mono); font-size:.7rem; letter-spacing:.04em; padding:1px 7px;
  border-radius:5px; font-weight:600}
.tag.agentic{color:var(--accent); background:var(--accent-soft); border:1px solid var(--accent-line)}
.tag.classic{color:var(--muted); background:var(--panel-2); border:1px solid var(--line)}

/* ---- E2–E8 ---- */
.evals{display:flex; flex-direction:column; gap:2px; border:1px solid var(--line);
  border-radius:var(--r); overflow:hidden; background:var(--panel)}
.eval{display:grid; grid-template-columns:72px 1fr; gap:20px; padding:20px 22px;
  border-bottom:1px solid var(--line)}
.eval:last-child{border-bottom:none}
.eval.head{background:var(--accent-soft)}
.eval-code{font-family:var(--mono); font-weight:700; font-size:1.05rem; color:var(--accent);
  letter-spacing:.03em}
.eval-top{display:flex; align-items:baseline; gap:14px; flex-wrap:wrap; margin-bottom:5px}
.eval-top h3{font-size:1.08rem; font-weight:640; letter-spacing:-.01em}
.metric{font-family:var(--mono); font-size:.78rem; color:var(--cool);
  background:var(--cool-soft); padding:2px 9px; border-radius:6px; letter-spacing:.01em}
.eval-body p{color:var(--ink-dim); font-size:.95rem; line-height:1.55; max-width:78ch}

/* ---- DOOM ---- */
.capstone .section-head .marker{color:var(--bad);
  background:color-mix(in srgb,var(--bad) 12%,transparent);
  border-color:color-mix(in srgb,var(--bad) 34%,transparent)}
.shots{display:grid; gap:14px; margin:8px 0 0}
.shots.two{grid-template-columns:repeat(2,1fr)}
.shots.three{grid-template-columns:repeat(3,1fr); margin-top:18px}
.shots figure{margin:0}
.shots img{width:100%; display:block; border-radius:10px; border:1px solid var(--line);
  background:#000; image-rendering:auto}
.shots figcaption{font-size:.8rem; color:var(--muted); margin-top:9px; line-height:1.4}
.fork{margin-top:34px; padding:24px; background:var(--panel); border:1px solid var(--line);
  border-radius:var(--r)}
.fork-lede{color:var(--ink-dim); font-size:1.0rem; max-width:82ch}
.fork .shots.three figcaption{font-family:var(--mono); font-size:.72rem; letter-spacing:.04em;
  text-transform:uppercase; text-align:center; color:var(--muted)}

/* ---- footer ---- */
.foot{border-top:1px solid var(--line); padding-top:40px; margin-top:16px}
.foot-grid{display:grid; grid-template-columns:1fr 1fr; gap:32px; margin-bottom:36px}
.foot-grid p:not(.eyebrow){color:var(--ink-dim); font-size:.95rem; margin-top:10px; max-width:52ch}
.sig{max-width:var(--measure); font-size:1.05rem; color:var(--ink); line-height:1.5}
.sig b{color:var(--accent); font-weight:640}

@keyframes rise{from{opacity:0; transform:translateY(14px)}to{opacity:1; transform:none}}
@keyframes grow{from{transform:scaleX(0); transform-origin:left}to{transform:none}}
@keyframes blink{50%{opacity:0}}

@media (max-width:820px){
  body{font-size:16px}
  .hero-grid{grid-template-columns:1fr; gap:32px}
  .foot-grid{grid-template-columns:1fr; gap:24px}
  .shots.three{grid-template-columns:1fr}
  .shots.two{grid-template-columns:1fr}
}
@media (prefers-reduced-motion:reduce){
  *{animation:none!important}
  h1::after,.cursor{animation:none}
}
</style>
"""

OUT.write_text(STYLE + HTML)
print(f"wrote {OUT} ({len(STYLE + HTML)//1024} KB)")
