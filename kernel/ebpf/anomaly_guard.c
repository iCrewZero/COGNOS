/*
 * anomaly_guard.c — eBPF runtime anomaly detector for COGNOS/OS.
 *
 * Purpose:
 *   Detects suspicious runtime patterns: syscall floods, execve storms, and
 *   connect storms. Each per-pid/per-agent counter is bucketed into a fixed
 *   1-second window; when the counter crosses a (v0: fixed) threshold an
 *   anomaly_alert is pushed to userspace via a ring buffer.
 *
 * Status:
 *   // v0: stub — fixed thresholds, no learning
 *   // TODO(v1): per-agent baseline learning, EWMA-smoothed thresholds,
 *   //           adaptive window sizing, multi-feature anomaly scoring.
 *
 * License: GPL-2.0
 * Author:  COGNOS/OS Team
 */

#include "vmlinux.h"
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>
#include <bpf/bpf_core_read.h>

char LICENSE[] SEC("license") = "GPL";

/* Alert kinds. */
#define ANOM_KIND_SYSCALL_FLOOD 1
#define ANOM_KIND_EXECVE_STORM  2
#define ANOM_KIND_CONNECT_STORM 3

/* Fixed v0 thresholds (events per second). */
#define THRESH_SYSCALL 10000ULL
#define THRESH_EXECVE  50ULL
#define THRESH_CONNECT 100ULL
#define WINDOW_NS      (1ULL * 1000 * 1000 * 1000)

/* Anomaly alert pushed to userspace. */
struct anomaly_alert {
    u64 ts;         /* timestamp (ns) */
    u32 pid;        /* kernel pid */
    u32 agent_id;   /* COGNOS agent id (0 if untracked) */
    u8  kind;       /* one of ANOM_KIND_* */
    u8  _pad[7];
    u64 rate;       /* observed rate (events/s) at detection time */
};

/* Per-pid syscall counter for the current 1-second window. */
struct rate_bucket {
    u64 count;       /* event count this window */
    u64 window_start; /* ns timestamp at which the window opened */
};

/* Per-pid syscall rate. */
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 8192);
    __type(key, u32);            /* pid */
    __type(value, struct rate_bucket);
} syscall_rate SEC(".maps");

/* Per-agent execve counter for the current window. Keyed on agent_id. */
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 1024);
    __type(key, u32);
    __type(value, struct rate_bucket);
} execve_rate SEC(".maps");

/* Per-agent connect counter for the current window. Keyed on agent_id. */
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 1024);
    __type(key, u32);
    __type(value, struct rate_bucket);
} connect_rate SEC(".maps");

/* pid -> agent_id mapping (mirrored from agent_monitor.c). */
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 1024);
    __type(key, u32);
    __type(value, u32);
} agent_procs SEC(".maps");

/* Per-CPU array storing the last window reset timestamp. Used to lazily
 * roll over windows when a CPU observes that WINDOW_NS has elapsed. */
struct {
    __uint(type, BPF_MAP_TYPE_PERCPU_ARRAY);
    __uint(max_entries, 1);
    __type(key, u32);
    __type(value, u64);
} last_reset_ns SEC(".maps");

/* Immediate alert delivery. */
struct {
    __uint(type, BPF_MAP_TYPE_RINGBUF);
    __uint(max_entries, 256 * 1024);
} anomaly_alert SEC(".maps");

/* Resolve the COGNOS agent_id for the current task. Returns 0 if untracked. */
static __always_inline u32 agent_of_pid(u32 pid)
{
    u32 *aid = bpf_map_lookup_elem(&agent_procs, &pid);
    return aid ? *aid : 0;
}

/* Lazily reset a per-(agent|pid) bucket when its window has elapsed.
 * Returns 1 if the bucket was just reset (so the caller starts at count=1). */
static __always_inline int maybe_reset(struct rate_bucket *b, u64 now)
{
    if (now - b->window_start >= WINDOW_NS) {
        b->count = 0;
        b->window_start = now;
        return 1;
    }
    return 0;
}

/* Helper to push an alert to the ring buffer. */
static __always_inline void emit_alert(u8 kind, u32 pid, u32 agent_id, u64 rate)
{
    struct anomaly_alert *a =
        bpf_ringbuf_reserve(&anomaly_alert, sizeof(*a), 0);
    if (!a)
        return;
    a->ts       = bpf_ktime_get_ns();
    a->pid      = pid;
    a->agent_id = agent_id;
    a->kind     = kind;
    a->rate     = rate;
    __builtin_memset(a->_pad, 0, sizeof(a->_pad));
    bpf_ringbuf_submit(a, 0);
}

/* Refresh the per-CPU global window-start timestamp. Used as a coarse
 * time-base so we do not pay the cost of bpf_ktime_get_ns() on every
 * counter lookup. */
static __always_inline u64 current_window_start(u64 now)
{
    u32 key = 0;
    u64 *reset = bpf_map_lookup_elem(&last_reset_ns, &key);
    if (!reset) {
        /* Should never happen for a per-cpu array of size 1, but be
         * defensive: initialize on first observation. */
        bpf_map_update_elem(&last_reset_ns, &key, &now, BPF_ANY);
        return now;
    }
    if (now - *reset >= WINDOW_NS)
        *reset = now;
    return *reset;
}

/* tracepoint/raw_syscalls/sys_enter — increment per-pid syscall counter;
 * if the rate crosses THRESH_SYSCALL for the current 1-second window emit
 * an anomaly_alert kind=1. */
SEC("tracepoint/raw_syscalls/sys_enter")
int BPF_PROG(sys_enter, struct pt_regs *regs, long id)
{
    u64 now = bpf_ktime_get_ns();
    u32 pid = (u32)bpf_get_current_pid_tgid();

    struct rate_bucket *b = bpf_map_lookup_elem(&syscall_rate, &pid);
    if (!b) {
        struct rate_bucket nb = { .count = 1, .window_start = now };
        bpf_map_update_elem(&syscall_rate, &pid, &nb, BPF_NOEXIST);
        return 0;
    }
    maybe_reset(b, now);
    __sync_fetch_and_add(&b->count, 1);

    if (b->count == THRESH_SYSCALL) {
        /* Emit exactly once when the threshold is crossed; subsequent
         * hits in the same window are silently counted. */
        emit_alert(ANOM_KIND_SYSCALL_FLOOD, pid, agent_of_pid(pid),
                   b->count);
    }
    return 0;
}

/* kprobe/__x64_sys_execve — count execve per agent. If the rate crosses
 * THRESH_EXECVE emit anomaly_alert kind=2. */
SEC("kprobe/__x64_sys_execve")
int BPF_KPROBE(kprobe_execve, struct pt_regs *regs)
{
    u64 now = bpf_ktime_get_ns();
    u32 pid = (u32)bpf_get_current_pid_tgid();
    u32 agent_id = agent_of_pid(pid);
    if (agent_id == 0)
        return 0;

    struct rate_bucket *b = bpf_map_lookup_elem(&execve_rate, &agent_id);
    if (!b) {
        struct rate_bucket nb = { .count = 1, .window_start = now };
        bpf_map_update_elem(&execve_rate, &agent_id, &nb, BPF_NOEXIST);
        return 0;
    }
    maybe_reset(b, now);
    __sync_fetch_and_add(&b->count, 1);

    if (b->count == THRESH_EXECVE)
        emit_alert(ANOM_KIND_EXECVE_STORM, pid, agent_id, b->count);
    return 0;
}

/* kprobe/tcp_connect — count connects per agent. If the rate crosses
 * THRESH_CONNECT emit anomaly_alert kind=3. The exact kernel symbol varies
 * across releases; the build system selects tcp_v4_connect on most kernels. */
SEC("kprobe/tcp_v4_connect")
int BPF_KPROBE(kprobe_tcp_connect, struct sock *sk, struct sockaddr *uaddr,
               int addr_len)
{
    u64 now = bpf_ktime_get_ns();
    u32 pid = (u32)bpf_get_current_pid_tgid();
    u32 agent_id = agent_of_pid(pid);
    if (agent_id == 0)
        return 0;

    struct rate_bucket *b = bpf_map_lookup_elem(&connect_rate, &agent_id);
    if (!b) {
        struct rate_bucket nb = { .count = 1, .window_start = now };
        bpf_map_update_elem(&connect_rate, &agent_id, &nb, BPF_NOEXIST);
        return 0;
    }
    maybe_reset(b, now);
    __sync_fetch_and_add(&b->count, 1);

    if (b->count == THRESH_CONNECT)
        emit_alert(ANOM_KIND_CONNECT_STORM, pid, agent_id, b->count);
    return 0;
}

/* v0: stub — fixed thresholds, no learning */
/* TODO(v1): replace fixed THRESH_* with per-agent baselines computed by the
 * userspace telemetry agent and pushed down as a map; add EWMA smoothing
 * over the rate bucket; combine syscall / execve / connect signals into a
 * single multi-feature anomaly score; add a slow-path that samples full
 * syscall histograms for forensic replay. */
