/* scheduler_telemetry.h — shared structs between eBPF kernel and Rust userspace */
#pragma once
#include <stdint.h>

struct sched_event {
    uint64_t timestamp_ns;
    uint32_t pid;
    uint32_t tgid;
    char     comm[16];
    uint64_t cpu_time_ns;
    uint32_t cpu_id;
    uint8_t  is_voluntary;
    uint8_t  _pad[3];
};

struct irq_stats {
    uint64_t count;
    uint64_t total_duration_ns;
    uint32_t cpu_id;
    uint32_t pad;
};

struct syscall_event {
    uint64_t timestamp_ns;
    uint32_t pid;
    uint32_t tgid;
    char     comm[16];
    uint32_t syscall_nr;
    char     path[128];
    uint32_t flags;
    uint8_t  is_sensitive;
    uint8_t  _pad[3];
};
