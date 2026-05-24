// SPDX-License-Identifier: GPL-2.0
// app_monitor.bpf.c — per-app syscall monitoring for Security Agent behavioral model.
// Attaches to openat, connect, execve, ptrace, mmap.

#include "vmlinux.h"
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>
#include <bpf/bpf_core_read.h>
#include "scheduler_telemetry.h"

char LICENSE[] SEC("license") = "GPL";

// Ring buffer: 8 MB for syscall events.
struct {
    __uint(type, BPF_MAP_TYPE_RINGBUF);
    __uint(max_entries, 8 * 1024 * 1024);
} syscall_events SEC(".maps");

// Userspace-controlled: only emit events for PIDs in this map.
// Key: pid, Value: 1 (presence means monitored).
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 1024);
    __type(key, u32);
    __type(value, u8);
} monitored_pids SEC(".maps");

static __always_inline int emit_event(
    u32 syscall_nr, const char *path, u32 path_len, u32 flags, u8 sensitive)
{
    u32 pid = (u32)bpf_get_current_pid_tgid();
    u8 *monitored = bpf_map_lookup_elem(&monitored_pids, &pid);
    if (!monitored) return 0;

    struct syscall_event *e = bpf_ringbuf_reserve(&syscall_events, sizeof(*e), 0);
    if (!e) return 0;

    e->timestamp_ns = bpf_ktime_get_ns();
    e->pid          = pid;
    e->tgid         = (u32)(bpf_get_current_pid_tgid() >> 32);
    e->syscall_nr   = syscall_nr;
    e->flags        = flags;
    e->is_sensitive = sensitive;
    bpf_get_current_comm(e->comm, sizeof(e->comm));

    if (path && path_len > 0) {
        bpf_probe_read_user_str(e->path, sizeof(e->path), path);
    } else {
        e->path[0] = '\0';
    }

    bpf_ringbuf_submit(e, 0);
    return 0;
}

// openat — file open events
SEC("tp/syscalls/sys_enter_openat")
int handle_openat(struct trace_event_raw_sys_enter *ctx)
{
    const char *filename = (const char *)ctx->args[1];
    return emit_event(257 /* SYS_openat */, filename, 256, (u32)ctx->args[2], 0);
}

// connect — network connections
SEC("tp/syscalls/sys_enter_connect")
int handle_connect(struct trace_event_raw_sys_enter *ctx)
{
    return emit_event(42 /* SYS_connect */, NULL, 0, 0, 0);
}

// execve — always sensitive
SEC("tp/syscalls/sys_enter_execve")
int handle_execve(struct trace_event_raw_sys_enter *ctx)
{
    const char *filename = (const char *)ctx->args[0];
    return emit_event(59 /* SYS_execve */, filename, 128, 0, 1 /* sensitive */);
}

// ptrace — always suspicious
SEC("tp/syscalls/sys_enter_ptrace")
int handle_ptrace(struct trace_event_raw_sys_enter *ctx)
{
    return emit_event(101 /* SYS_ptrace */, NULL, 0, (u32)ctx->args[0], 1);
}

// mmap — only emit when PROT_EXEC is set
SEC("tp/syscalls/sys_enter_mmap")
int handle_mmap(struct trace_event_raw_sys_enter *ctx)
{
    u32 prot = (u32)ctx->args[2];
    if (!(prot & 0x4 /* PROT_EXEC */)) return 0;
    return emit_event(9 /* SYS_mmap */, NULL, 0, prot, 1 /* sensitive */);
}
