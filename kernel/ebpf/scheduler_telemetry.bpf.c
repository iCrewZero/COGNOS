// SPDX-License-Identifier: GPL-2.0
// scheduler_telemetry.bpf.c — feeds per-process CPU metrics to the AI scheduler.
// Attaches to sched_switch, sched_process_exec, sched_process_exit.

#include "vmlinux.h"
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>
#include <bpf/bpf_core_read.h>
#include "scheduler_telemetry.h"

char LICENSE[] SEC("license") = "GPL";

// Ring buffer: 4 MB, events dropped (not blocking) when full.
struct {
    __uint(type, BPF_MAP_TYPE_RINGBUF);
    __uint(max_entries, 4 * 1024 * 1024);
} sched_events SEC(".maps");

// Per-CPU array: tracks cumulative CPU time per PID.
struct {
    __uint(type, BPF_MAP_TYPE_PERCPU_HASH);
    __uint(max_entries, 10240);
    __type(key, u32);          // pid
    __type(value, u64);        // start time ns on CPU
} pid_start_time SEC(".maps");

// Drop counter: userspace reads this to detect telemetry gaps.
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __uint(max_entries, 1);
    __type(key, u32);
    __type(value, u64);
} drop_counter SEC(".maps");

SEC("tp/sched/sched_switch")
int handle_sched_switch(struct trace_event_raw_sched_switch *ctx)
{
    u64 ts = bpf_ktime_get_ns();
    u32 prev_pid = ctx->prev_pid;
    u32 next_pid = ctx->next_pid;
    u32 cpu = bpf_get_smp_processor_id();

    // Record how long the outgoing process ran.
    u64 *start = bpf_map_lookup_elem(&pid_start_time, &prev_pid);
    u64 cpu_time_ns = 0;
    if (start && *start > 0) {
        cpu_time_ns = ts - *start;
        bpf_map_delete_elem(&pid_start_time, &prev_pid);
    }

    // Record start time for incoming process.
    bpf_map_update_elem(&pid_start_time, &next_pid, &ts, BPF_ANY);

    // Only emit events for the outgoing process (it just finished a slice).
    if (prev_pid == 0) return 0; // skip idle

    struct sched_event *e = bpf_ringbuf_reserve(&sched_events, sizeof(*e), 0);
    if (!e) {
        // Increment drop counter.
        u32 key = 0;
        u64 *drops = bpf_map_lookup_elem(&drop_counter, &key);
        if (drops) __sync_fetch_and_add(drops, 1);
        return 0;
    }

    e->timestamp_ns = ts;
    e->pid          = prev_pid;
    e->tgid         = (u32)(bpf_get_current_pid_tgid() >> 32);
    e->cpu_id       = cpu;
    e->cpu_time_ns  = cpu_time_ns;
    e->is_voluntary = (ctx->prev_state & TASK_INTERRUPTIBLE) != 0 ? 1 : 0;
    bpf_get_current_comm(e->comm, sizeof(e->comm));

    bpf_ringbuf_submit(e, 0);
    return 0;
}

SEC("tp/sched/sched_process_exec")
int handle_exec(struct trace_event_raw_sched_process_exec *ctx)
{
    u64 ts = bpf_ktime_get_ns();
    u32 pid = ctx->pid;
    bpf_map_update_elem(&pid_start_time, &pid, &ts, BPF_ANY);
    return 0;
}

SEC("tp/sched/sched_process_exit")
int handle_exit(struct trace_event_raw_sched_process_exit *ctx)
{
    u32 pid = ctx->pid;
    bpf_map_delete_elem(&pid_start_time, &pid);
    return 0;
}
