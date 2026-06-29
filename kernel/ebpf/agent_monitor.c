/*
 * agent_monitor.c — eBPF per-agent process monitoring for COGNOS/OS.
 *
 * Purpose:
 *   Tracks the activity of agent-owned processes (file opens, writes, network
 *   connects, execve) and emits raw events to a ring buffer that is consumed
 *   by the Security Agent behavioral model. The mapping between a kernel pid
 *   and a COGNOS agent_id is established at agent spawn time by the
 *   orchestrator via bpf_map_update_elem on agent_procs.
 *
 * Status:
 *   // v0: stub — emits raw events, no rate limiting
 *   // TODO(v1): per-agent token-bucket rate limiter, event coalescing,
 *   //           pathname canonicalization, cgroup-based tracking.
 *
 * License: GPL-2.0
 * Author:  COGNOS/OS Team
 */

#include "vmlinux.h"
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>
#include <bpf/bpf_core_read.h>

char LICENSE[] SEC("license") = "GPL";

/* Event kind enum: 1=open, 2=write, 3=connect, 4=execve. Kept as a u8 in
 * the wire struct so userspace can extend without reflowing the schema. */
#define AGENT_KIND_OPEN    1
#define AGENT_KIND_WRITE   2
#define AGENT_KIND_CONNECT 3
#define AGENT_KIND_EXECVE  4

/* Single event emitted to userspace. */
struct agent_event {
    u64  ts;         /* timestamp (ns) */
    u32  pid;        /* kernel pid */
    u32  agent_id;   /* COGNOS agent id (0 if untracked) */
    u8   kind;       /* one of AGENT_KIND_* */
    u8   _pad[7];
    u64  arg0;       /* syscall arg0 (fd, flags, etc.) */
    u64  arg1;       /* syscall arg1 */
    char path[64];   /* pathname / addr string */
};

/* pid -> agent_id mapping. Populated from userspace (the orchestrator) when
 * an agent process is spawned. A lookup miss means "untracked" and the
 * tracepoints skip emission. */
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 1024);
    __type(key, u32);    /* pid */
    __type(value, u32);  /* agent_id */
} agent_procs SEC(".maps");

/* Ring buffer: 256 KiB. Userspace drains this with ring_buffer__poll(). */
struct {
    __uint(type, BPF_MAP_TYPE_RINGBUF);
    __uint(max_entries, 256 * 1024);
} agent_events SEC(".maps");

/* Resolve the COGNOS agent_id for the current task. Returns 0 if untracked. */
static __always_inline u32 agent_of_pid(u32 pid)
{
    u32 *aid = bpf_map_lookup_elem(&agent_procs, &pid);
    return aid ? *aid : 0;
}

/* Core emit helper: only emits if the current pid is tracked. */
static __always_inline int emit_agent_event(u8 kind, u64 arg0, u64 arg1,
                                            const char *user_path)
{
    u64 pid_tgid = bpf_get_current_pid_tgid();
    u32 pid = (u32)pid_tgid;
    u32 agent_id = agent_of_pid(pid);
    if (agent_id == 0)
        return 0;

    struct agent_event *e =
        bpf_ringbuf_reserve(&agent_events, sizeof(*e), 0);
    if (!e)
        return 0;

    e->ts       = bpf_ktime_get_ns();
    e->pid      = pid;
    e->agent_id = agent_id;
    e->kind     = kind;
    e->arg0     = arg0;
    e->arg1     = arg1;
    __builtin_memset(e->_pad, 0, sizeof(e->_pad));

    if (user_path) {
        /* bpf_probe_read_user_str returns bytes including the NUL; truncate
         * silently on overflow. */
        bpf_probe_read_user_str(e->path, sizeof(e->path), user_path);
    } else {
        e->path[0] = '\0';
    }

    bpf_ringbuf_submit(e, 0);
    return 0;
}

/* tracepoint/syscalls/sys_enter_openat — file open events. */
SEC("tracepoint/syscalls/sys_enter_openat")
int BPF_PROG(sys_enter_openat, int dfd, const char *filename,
             int flags, umode_t mode)
{
    return emit_agent_event(AGENT_KIND_OPEN, (u64)(u32)flags,
                            (u64)(u32)mode, filename);
}

/* tracepoint/syscalls/sys_enter_write — file write events. arg0 is the fd,
 * arg1 is the user buffer length so the behavioral model can spot large or
 * suspicious writes. */
SEC("tracepoint/syscalls/sys_enter_write")
int BPF_PROG(sys_enter_write, unsigned int fd, const char *buf,
             size_t count)
{
    return emit_agent_event(AGENT_KIND_WRITE, (u64)fd, (u64)count, NULL);
}

/* tracepoint/syscalls/sys_enter_connect — network connect events. The
 * sockaddr is left to userspace to decode; we just record the syscall args. */
SEC("tracepoint/syscalls/sys_enter_connect")
int BPF_PROG(sys_enter_connect, int fd, struct sockaddr *uservaddr,
             int addrlen)
{
    return emit_agent_event(AGENT_KIND_CONNECT, (u64)(u32)fd,
                            (u64)(u32)addrlen, NULL);
}

/* tracepoint/syscalls/sys_enter_execve — execve events. Always emitted for
 * tracked agents, since spawning a subprocess is high-signal. */
SEC("tracepoint/syscalls/sys_enter_execve")
int BPF_PROG(sys_enter_execve, const char *filename, const char *const *argv,
             const char *const *envp)
{
    return emit_agent_event(AGENT_KIND_EXECVE, 0, 0, filename);
}

/* cgroup/track_agent — userspace-driven registration hook. The orchestrator
 * writes a {pid -> agent_id} entry into agent_procs by invoking
 * bpf_map_update_elem directly; this program exists so future v1 work can
 * attach side effects (e.g. tagging the cgroup) at registration time. */
SEC("cgroup/track_agent")
int BPF_PROG(track_agent, struct cgroup *cgrp, const char *comm)
{
    /* v0: no-op; the actual mapping is created from userspace. This stub
     *      is intentionally a no-op so the SEC slot exists for future use. */
    return 0;
}

/* v0: stub — emits raw events, no rate limiting */
/* TODO(v1): per-agent token-bucket rate limiter keyed on agent_id, event
 * coalescing for repeated opens of the same path, canonicalization of
 * symlinked paths, and cgroup-tree-wide tracking so child processes do not
 * need a separate userspace registration call. */
