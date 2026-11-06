# MEMORY.md — Memory management

> The strategy for **one thing**: keeping the whole process inside the memory box we
> promise, on a 512 MB VPS, under any traffic.
>
> AGENTS.md is the source of truth for the design; this file is the full statement of
> the memory half of it. The user-facing version of the same story is
> [`service/docs/memory.html`](service/docs/memory.html) (served at `/docs/memory.html`,
> published at <https://sumimi-dev.github.io>).
>
> Open questions live in issues, not here: [#12](https://github.com/sumimi-dev/sumimi/issues/12)
> (the two big problems), [#13](https://github.com/sumimi-dev/sumimi/issues/13) (how DuckDB
> really manages memory), [#14](https://github.com/sumimi-dev/sumimi/issues/14) (what exactly
> decides the collector's peak).

---

## 1. The promise

| tier | the machine | the engine's ceiling | **the whole process** must stay under |
| --- | --- | ---: | ---: |
| **minimum** | 1 vCPU / 512 MB | 128 MB | **256 MB** |
| **recommended** | 1 vCPU / 1 GB | 256 MB | **512 MB** |

Two things are true about that table, and the second one is the one people forget:

1. **The number that matters is the process's peak RSS, not the engine's.** What a small
   box runs out of is *everything*: the code pages, the collector's strings, the queue, the
   pages the allocator keeps, and the engine's share.
2. **The engine gets a quarter of the machine, not the whole of it.** The other three
   quarters belong to the operating system, to the collector, to the queue and to the
   allocator — and until recently **nobody was budgeting for those at all**.

The promise is asserted, not hoped for: the daily CI job reads the exact high-water mark
(`/proc/self/status`, `VmHWM`) on Linux and fails the build when the process outgrows its
box.

## 2. The ledger: six accounts, and who bounds each one

| account | what it is | bounded by | measured |
| --- | --- | --- | --- |
| the binary and the stacks | the executable's text and data, the thread stacks | nothing (it is what it is) | 42 MB idle |
| **the engine's buffer pool** | DuckDB's own buffer manager | `memory_limit` — `min(RAM/4, 256 MB)` | 0.5–7 MB |
| **the engine's other allocations** | the appender's rows, the WAL, DuckDB's internal allocator | **nothing we set** | unknown — see [#13](https://github.com/sumimi-dev/sumimi/issues/13) |
| **the collector** | the HTTP collection path: the body, the JSON tree, the parsed hit | the admission gate (§5) | **the rate is the lever** — see §5 |
| **the queue** | the hits waiting to be written | `SUMIMI_MAX_QUEUE` = 100 000 **rows** | ~5 MB of structs per 10 000 rows, plus the strings |
| **the allocator's retention** | the pages glibc took and did not give back | **nothing** ⚠ | the peak never comes back down |

`duckdb_memory()` reports the **buffer manager only**. It is not the engine's total and it
is not the process's — reading it as either is how a 2 MB engine and a 636 MB process end
up in the same sentence.

## 3. The facts we have

| | peak RSS | the engine holds | the queue | where it was measured |
| --- | ---: | ---: | ---: | --- |
| idle | 42 MB | 0.5 MB | 0 | the daily budget run |
| the write path, 200 000 events | **168 MB** (128 MB engine) / **172 MB** (256 MB engine) | 7 MB | 0 | the daily budget run, Linux, exact peak |
| a burst: 1 GB injected in 60 s | **636 MB** | 2 MB | 0 at the peak | the soak (macOS, sampled) |
| the same burst, another day | **1560 MB** | 4 MB | 0 at the peak | the soak (macOS, sampled) |

Three things follow, and they are the whole story:

* **The engine is innocent.** 2–7 MB, in every run, at every traffic shape. DuckDB's own
  strategy works; the problem is that we never wrote a strategy around it.
* **The burst is the problem, and it is ours.** At the peak the queue is empty and the
  engine holds 2 MB — so ~600 MB is the collector's own allocations and the pages the
  allocator kept.
* **The same shape gave 636 MB and 1560 MB.** Variance like that means *one run is not a
  measurement* — every number in this file has to come from a median of at least three
  runs, on Linux, where the peak is exact rather than sampled.

## 4. The strategy: five lines, around the engine's own

DuckDB has a strategy of its own, and the rule is to **design around it, not against it**:

| DuckDB's rule | what it means for us |
| --- | --- |
| `memory_limit` bounds the **buffer pool**, not the data | the engine's share is a quarter of the machine, and we leave the rest alone |
| a query **streams and spills** to disk | a long query does not need the box's RAM, it needs the box's disk |
| the buffer is **flushed** back (`allocator_flush_threshold`, 32 MB) | DuckDB gives its own memory back; we have to give ours back too |
| **one thread per 64 MB** of budget | a parallel aggregate builds one hash table per thread, so the thread count *is* a memory rule |

Our five lines are the same shape — one per account in §2:

| | line | state |
| --- | --- | --- |
| **G1** | the engine's ceiling — `min(RAM/4, 256 MB)` | ✅ in place |
| **G2** | the write batch — `SUMIMI_MAX_BATCH` = 10 000 rows | ✅ in place, but counted in **rows**, not bytes |
| **G3** | **the admission gate before the queue** — nothing may be parsed without a bound | ✅ **built** (`service/src/limits.rs`) — the knobs exist and are adjustable; the **defaults are still off** until the numbers are settled |
| **G4** | the queue's capacity — `SUMIMI_MAX_QUEUE` = 100 000 rows | ⚠ in place, but 100 000 rows is ~50 MB of structs **plus the strings**, and the strings are the bigger half |
| **G5** | giving the allocator's pages back | ❌ missing (needs `malloc_trim`, which needs §5 rule 1 relaxed) |

### The invariants

These are the rules that make the promise true. A change that breaks one is a change that
breaks the product.

1. **Every allocation is bounded by something we set.** No path may allocate "as much as
   the traffic brings".
2. **A refusal must happen before the allocation it refuses.** Refusing after the work is
   done is not a limit; it is a delay.
3. **The engine gets a quarter of the machine; the rest of the process has its own
   budget**, and the two budgets are spent separately.
4. **The peak, not the average.** A budget is about the high-water mark: the moment the
   process is largest is the moment the box dies.
5. **Every number is measured on Linux**, where the peak is exact, and it comes from at
   least three runs.

## 5. The collection path: where the rate limit goes

This is the part that is missing (G3), and it is the part that produced the 636 MB.

### What happens today

```
POST /api/send
  └─ axum's Json extractor      reads the whole body into memory  (default limit: 2 MB)
     └─ serde_json::from_slice   a JSON tree, one allocation per string
        └─ into_hit()            flat = obj.clone()      ← a second copy
           └─ build_hit()        ~30 Option<String>     ← a third copy
              └─ queue.push()     the hit waits here
                 └─ writer        a batch of ≤10 000 rows is written to DuckDB
```

Three copies of the same strings are alive at once, and the queue can hold 100 000 hits.
**Nothing on this path has a limit of its own** — the only gate is the queue's row count,
and it is checked *after* the body has been read and the JSON has been parsed.

### The two problems

1. **There is no bound.** The collector accepts as much as the network brings, and the
   allocator's arenas grow to the high-water mark of whatever burst arrived.
2. **The refusal is too late.** The queue-full `503` is raised after the parse. The memory
   has already been spent by then, so refusing protects the *queue*, not the *box*.

### The target

```
POST /api/send
  └─ read the headers only         nothing is allocated yet
     └─ ADMIT                      ← the gate, before the body is read
        ├─ is the body within the per-request limit?
        ├─ does the in-flight byte budget have room for it?
        ├─ are we under the in-flight request limit?
        └─ is there a rate token?
     └─ read the body (bounded)     only now does a byte of it exist
        └─ parse (one copy)         stream it; do not build three copies
           └─ queue.push()          the queue is bounded in bytes
```

Four knobs, and each one is a different kind of limit:

| knob | environment variable | bounds | why it is not the others |
| --- | --- | --- | --- |
| **the per-request body limit** | `SUMIMI_MAX_BODY_BYTES` | one request | a single request must never be able to blow the budget |
| **the in-flight byte budget** | `SUMIMI_MAX_INFLIGHT_BYTES` | the collector's live working set | **bytes, not requests** — a request is not a memory unit |
| **the in-flight request limit** | `SUMIMI_MAX_INFLIGHT_REQUESTS` | the number of parsers | bounds the number of arenas the allocator touches |
| **the admission rate** | `SUMIMI_MAX_ADMIT_RPS` | requests per second | bounds the churn — the allocator's high-water mark grows with the *rate*, not with the total |

**Zero means "no limit"**, and that is how the defaults are: the gate changes nothing
until somebody turns it on, and the numbers it gets are the ones the measurements
choose — not the ones a hunch chooses.

### The rate follows the writer

The fourth knob is not a constant, and this is the part that makes it "our own strategy"
rather than a guess:

* the writer knows how fast it drains the queue (rows per second, measured over the last
  few flushes);
* **the admission rate is set to the drain rate** (times a small safety factor) — never
  faster, because accepting hits we cannot write only fills the queue and ends in a
  refusal we could have avoided;
* when the queue is above a watermark, the admission rate drops and the answers carry a
  `Retry-After` computed from the drain rate — **backpressure instead of a wall**;
* the client (our tracker, and Umami's) already retries a `503`, so a refusal is a
  slowdown, not a loss. What we acknowledged, we have.

### The numbers are not decided yet

The *shape* of the gate is decided here; the *numbers* are not, because they depend on
which mechanism actually decides the peak — the rate, the in-flight volume, the live queue
or the engine's non-buffer-pool allocations. That is what
[#14](https://github.com/sumimi-dev/sumimi/issues/14) is measuring, one variable at a time.

The numbers will be derived, not guessed: **the in-flight byte budget is a share of the
tier's non-engine budget** (256 − 128 = 128 MB on the minimum tier), and the queue's
capacity in rows follows from the byte budget and the measured size of one queued hit.

### What the first sweep measured

The sweep (`pro/examples/gate_sweep.rs` — one variable at a time, three runs per
configuration, one process per run, the median) says something very plain:

| configuration | the peak RSS |
| --- | ---: |
| no gate (today) | **659 MB** |
| the body limit (64 KB) | 592 MB |
| the in-flight bytes (4 MB) | 594 MB |
| the in-flight requests (16) | 605 MB |
| **the rate (20 000/s)** | **188 MB** |
| **the rate (5 000/s)** | **95 MB** |
| all of them together | 189 MB |

(200 MB of payload over 20 s, 64 connections. Sampled on macOS — a lower bound; the CI
runs the same sweep on Linux, where the peak is exact.)

**The rate is the lever.** The other three knobs do nothing measurable on their own,
because the peak follows the **rate of allocation** — the allocator's churn — and not
the volume in flight. And the rate limit fixed a second thing for free: the queue never
filled once (0 dropped, against 6 233 without it).

So the strategy is: **limit the rate first**, and set the rate from what the writer can
actually drain — not from a constant. The in-flight limits stay as the backstop.

## 6. Where each piece is decided

| question | where |
| --- | --- |
| the promise, the ledger, the invariants | this file, and [AGENTS.md §4.1a](AGENTS.md) |
| how DuckDB manages its own memory | [#13](https://github.com/sumimi-dev/sumimi/issues/13) |
| what decides the collector's peak | [#14](https://github.com/sumimi-dev/sumimi/issues/14) |
| the two big problems, and the disk half | [#12](https://github.com/sumimi-dev/sumimi/issues/12) |
| the memory budget in the private edition | [sumimi-pro#5](https://github.com/sumimi-dev/sumimi-pro/issues/5) |
| the disk half of the private edition | [sumimi-pro#8](https://github.com/sumimi-dev/sumimi-pro/issues/8) |
