/*
 * capability_tracker.c — eBPF capability use tracker for COGNOS/OS.
 *
 * Purpose:
 *   Counts Linux capability checks (cap_capable / security_capable) per
 *   (agent_id, capability) pair and immediately reports violations through a
 *   ring buffer. The orchestrator's userspace polls the counter map for slow
 *   telemetry and consumes the ringbuf for fast alerting. bpf() syscall
 *   attempts originating from a tracked agent are logged via the LSM hook
 *   for later enforcement.
 *
 * Status:
 *   // v0: stub — counts only, no policy enforcement
 *   // TODO(v1): per-capability allow/deny policy table, denied capability
 *   //           alerts with audit chain provenance, cross-agent correlation.
 *
 * License: GPL-2.0
 * Author:  COGNOS/OS Team
 */

#include "vmlinux.h"
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>
#include <bpf/bpf_core_read.h>

char LICENSE[] SEC("license") = "GPL";

/* Composite key: (agent_id, capability). Mirrors a userspace struct; the
 * kernel treats it as an opaque 8-byte blob thanks to __type(). */
struct cap_key {
    u32 agent_id;
    u32 capability;
};

/* Violation record pushed to userspace. */
struct cap_violation {
    u64 ts;          /* timestamp (ns) */
    u32 pid;         /* kernel pid of the offender */
    u32 agent_id;    /* COGNOS agent id (0 if untracked) */
    u32 capability;  /* CAP_* numeric value */
    u32 audited;     /* 1 if the check was audited (security_capable) */
};

/* pid -> agent_id mapping. Same schema as in agent_monitor.c; kept
 * duplicated so this program can be loaded standalone without a dependency. */
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 1024);
    __type(key, u32);    /* pid */
    __type(value, u32);  /* agent_id */
} agent_procs SEC(".maps");

/* Aggregated counter: userspace polls this map every N ms to compute rates. */
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 4096);
    __type(key, struct cap_key);
    __type(value, u64);
} cap_counter SEC(".maps");

/* Immediate violation delivery. */
struct {
    __uint(type, BPF_MAP_TYPE_RINGBUF);
    __uint(max_entries, 256 * 1024);
} cap_violation SEC(".maps");

/* Resolve the COGNOS agent_id for the current task. Returns 0 if untracked. */
static __always_inline u32 agent_of_pid(u32 pid)
{
    u32 *aid = bpf_map_lookup_elem(&agent_procs, &pid);
    return aid ? *aid : 0;
}

/* Core accounting routine: look up the responsible agent, bump the
 * (agent_id, capability) counter, and optionally push a violation record. */
static __always_inline int account_cap(u32 capability, int audited)
{
    u64 pid_tgid = bpf_get_current_pid_tgid();
    u32 pid = (u32)pid_tgid;
    u32 agent_id = agent_of_pid(pid);
    if (agent_id == 0)
        return 0;

    struct cap_key key = {
        .agent_id = agent_id,
        .capability = capability,
    };

    u64 *cnt = bpf_map_lookup_elem(&cap_counter, &key);
    if (cnt) {
        __sync_fetch_and_add(cnt, 1);
    } else {
        u64 one = 1;
        bpf_map_update_elem(&cap_counter, &key, &one, BPF_NOEXIST);
    }

    /* v0: every audited check is reported as a "violation" so the
     *      userspace policy engine can decide what is actually allowed.
     *      Filtering / policy enforcement is deferred to v1. */
    if (audited) {
        struct cap_violation *v =
            bpf_ringbuf_reserve(&cap_violation, sizeof(*v), 0);
        if (!v)
            return 0;
        v->ts         = bpf_ktime_get_ns();
        v->pid        = pid;
        v->agent_id   = agent_id;
        v->capability = capability;
        v->audited    = 1;
        bpf_ringbuf_submit(v, 0);
    }

    return 0;
}

/* kprobe/cap_capable — primary capability check path in the kernel. */
SEC("kprobe/cap_capable")
int BPF_KPROBE(cap_capable, const struct cred *cred,
               struct user_namespace *targ_ns, int cap, unsigned int opts)
{
    return account_cap((u32)cap, 0);
}

/* kprobe/security_capable — LSM audited path. Same accounting, but flagged
 * as audited so userspace knows a security hook observed the check. */
SEC("kprobe/security_capable")
int BPF_KPROBE(security_capable, const struct cred *cred,
               struct user_namespace *targ_ns, int cap, unsigned int opts)
{
    return account_cap((u32)cap, 1);
}

/* lsm/bpf — log bpf() syscall attempts. In v0 this program only records
 * the attempt; it does not deny. v1 will deny attempts originating from
 * tracked agent processes by returning -EPERM. */
SEC("lsm/bpf")
int BPF_PROG(lsm_bpf, int cmd, union bpf_attr *attr, unsigned int size)
{
    u64 pid_tgid = bpf_get_current_pid_tgid();
    u32 pid = (u32)pid_tgid;
    u32 agent_id = agent_of_pid(pid);
    if (agent_id == 0)
        return 0;

    struct cap_violation *v =
        bpf_ringbuf_reserve(&cap_violation, sizeof(*v), 0);
    if (!v)
        return 0;
    /* Encode the bpf() cmd in the capability field; audited=2 distinguishes
     * LSM/bpf events from security_capable events. */
    v->ts         = bpf_ktime_get_ns();
    v->pid        = pid;
    v->agent_id   = agent_id;
    v->capability = (u32)cmd;
    v->audited    = 2;
    bpf_ringbuf_submit(v, 0);

    /* v0: pass-through. v1: return -EPERM for tracked agents. */
    return 0;
}

/* v0: stub — counts only, no policy enforcement */
/* TODO(v1): add an allow/deny policy map (cap_key -> verdict), enforce
 * verdicts by returning -EPERM from the LSM hook, attach audit-chain
 * provenance to every violation, and cross-correlate cap use with the
 * agent_monitor events to detect privilege-escalation patterns. */
