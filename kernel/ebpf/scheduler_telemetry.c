/*
 * scheduler_telemetry.c — eBPF scheduler telemetry for the COGNOS/OS AI scheduler.
 *
 * Purpose:
 *   Collects per-task scheduler events (context switches, wakeups, on-CPU
 *   durations, runqueue latency) and exports aggregated samples to userspace
 *   via a hash map. The userspace AI scheduler policy consumes these samples
 *   to make predictive placement and preemption decisions.
 *
 * Status:
 *   // v0: stub — collects samples, no aggregation logic yet
 *   // TODO(v1): per-cpu runqueue length, cgroup-aware aggregation,
 *   //           exponential decay smoothing, weighted oncpu averages.
 *
 * License: GPL-2.0
 * Author:  COGNOS/OS Team
 */

#include "vmlinux.h"
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>
#include <bpf/bpf_core_read.h>

char LICENSE[] SEC("license") = "GPL";

/* Per-task sample exported to userspace. Mirrors a userspace struct; keep in
 * sync with the Rust crate that polls sched_events. */
struct sched_sample {
    u64 ts;          /* timestamp (ns) of the event */
    u64 nvcsw;       /* cumulative voluntary context switches */
    u64 nivcsw;      /* cumulative involuntary context switches */
    u64 oncpu_ns;    /* on-CPU duration since last switch (ns) */
    u64 runqlat_ns;  /* time spent on runqueue before being scheduled (ns) */
    u32 pid;         /* kernel pid (thread id) */
    u32 tgid;        /* userspace tgid (process id) */
    char comm[16];   /* executable name */
};

/* Aggregated per-pid sample map. Userspace polls this hash map to read the
 * latest counters for each tracked task. */
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 8192);
    __type(key, u32);
    __type(value, struct sched_sample);
} sched_events SEC(".maps");

/* Ring buffer used to push high-signal events (e.g. long runqueue latency
 * spikes) to userspace immediately rather than waiting for a poll. */
struct {
    __uint(type, BPF_MAP_TYPE_RINGBUF);
    __uint(max_entries, 256 * 1024);
} sched_ringbuf SEC(".maps");

/* Records the timestamp at which a task was enqueued on a runqueue. Used to
 * compute runqueue latency at wakeup->switch time. */
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 8192);
    __type(key, u32);    /* pid */
    __type(value, u64);  /* enqueue timestamp ns */
} enq_ts SEC(".maps");

/* Records the timestamp at which a task started running on a CPU. Used to
 * compute on-CPU duration at the next context switch. */
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 8192);
    __type(key, u32);    /* pid */
    __type(value, u64);  /* on-cpu start timestamp ns */
} oncpu_start SEC(".maps");

/* Helper: fetch (or create) the sched_sample for the given pid. Returns NULL
 * if the hash map is full. */
static __always_inline struct sched_sample *get_or_create_sample(u32 pid)
{
    struct sched_sample *s = bpf_map_lookup_elem(&sched_events, &pid);
    if (s)
        return s;

    struct sched_sample zero = {};
    zero.pid = pid;
    bpf_map_update_elem(&sched_events, &pid, &zero, BPF_NOEXIST);
    return bpf_map_lookup_elem(&sched_events, &pid);
}

/* tracepoint/sched/sched_wakeup — record enqueue time for the woken task.
 * The task is about to become runnable; we stash the timestamp so that the
 * next sched_switch for this pid can compute runqueue latency. */
SEC("tracepoint/sched/sched_wakeup")
int BPF_PROG(sched_wakeup, struct task_struct *p)
{
    if (!p)
        return 0;
    u32 pid = BPF_CORE_READ(p, pid);
    if (pid == 0)
        return 0;
    u64 ts = bpf_ktime_get_ns();
    bpf_map_update_elem(&enq_ts, &pid, &ts, BPF_ANY);
    return 0;
}

/* tracepoint/sched/sched_switch — main telemetry hook. Records the on-CPU
 * duration of the outgoing task and the runqueue latency of the incoming
 * task, then aggregates both into the per-pid sched_events hash map. */
SEC("tracepoint/sched/sched_switch")
int BPF_PROG(sched_switch, bool preempt, struct task_struct *prev,
             struct task_struct *next)
{
    u64 ts = bpf_ktime_get_ns();
    u32 prev_pid = prev ? BPF_CORE_READ(prev, pid) : 0;
    u32 next_pid = next ? BPF_CORE_READ(next, pid) : 0;

    /* Close out the previous task's on-CPU slice. */
    if (prev_pid != 0) {
        u64 *start = bpf_map_lookup_elem(&oncpu_start, &prev_pid);
        if (start) {
            u64 delta = ts - *start;
            struct sched_sample *s = get_or_create_sample(prev_pid);
            if (s) {
                s->ts = ts;
                s->oncpu_ns = delta;
                s->pid = prev_pid;
                s->tgid = BPF_CORE_READ(prev, tgid);
                if (preempt)
                    s->nivcsw += 1;
                else
                    s->nvcsw += 1;
                BPF_CORE_READ_STR_INTO(&s->comm, prev, comm);
            }
            bpf_map_delete_elem(&oncpu_start, &prev_pid);
        }
    }

    /* Compute runqueue latency for the incoming task. */
    if (next_pid != 0) {
        u64 *enq = bpf_map_lookup_elem(&enq_ts, &next_pid);
        if (enq) {
            u64 lat = ts - *enq;
            struct sched_sample *s = get_or_create_sample(next_pid);
            if (s) {
                s->ts = ts;
                s->runqlat_ns = lat;
                s->pid = next_pid;
                /* If the latency crosses a (v0 fixed) threshold, push an
                 * immediate alert through the ringbuf so the userspace
                 * scheduler does not have to wait for the next poll. */
                if (lat > 10 * 1000 * 1000ULL) { /* 10 ms */
                    struct sched_sample *e =
                        bpf_ringbuf_reserve(&sched_ringbuf,
                                            sizeof(*e), 0);
                    if (e) {
                        __builtin_memcpy(e, s, sizeof(*e));
                        bpf_ringbuf_submit(e, 0);
                    }
                }
            }
            bpf_map_delete_elem(&enq_ts, &next_pid);
        }
        bpf_map_update_elem(&oncpu_start, &next_pid, &ts, BPF_ANY);
    }

    return 0;
}

/* kprobe/finish_task_switch — secondary on-CPU measurement path. The
 * tracepoint above already covers the common case; this kprobe exists so
 * that on-CPU duration is still measured for tasks that finish via the
 * finish_task_switch slow path (e.g. across migration between CPUs). */
SEC("kprobe/finish_task_switch")
int BPF_KPROBE(finish_task_switch, struct task_struct *prev)
{
    if (!prev)
        return 0;
    u32 prev_pid = BPF_CORE_READ(prev, pid);
    if (prev_pid == 0)
        return 0;
    u64 ts = bpf_ktime_get_ns();
    /* If oncpu_start has no entry for prev_pid the tracepoint already
     * recorded the slice; just refresh the start time defensively. */
    bpf_map_update_elem(&oncpu_start, &prev_pid, &ts, BPF_ANY);
    return 0;
}

/* v0: stub — collects samples, no aggregation logic yet */
/* TODO(v1): add per-cpu runqueue length, cgroup-scoped rollups, EWMA
 * smoothing of oncpu_ns and runqlat_ns, and predictive hints exported to
 * the AI scheduler through a dedicated ringbuf schema. */
