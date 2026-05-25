#!/usr/bin/env fish
# ~/.config/fish/completions/cognos.fish
# Tab completions for the cognos CLI

# Top-level subcommands
set -l cognos_cmds install remove update search list info \
    memory audit predict model agent noprotect cache version help

complete -c cognos -f -n "__fish_use_subcommand" -a "$cognos_cmds"

# memory subcommands
complete -c cognos -n "__fish_seen_subcommand_from memory" -f \
    -a "show wipe forget audit scope"
complete -c cognos -n "__fish_seen_subcommand_from memory" \
    -l scope -d "Filter by scope/domain"
complete -c cognos -n "__fish_seen_subcommand_from memory" \
    -l list -d "List indexed file count by domain"

# audit subcommands
complete -c cognos -n "__fish_seen_subcommand_from audit" -f \
    -a "show verify wipe export"
complete -c cognos -n "__fish_seen_subcommand_from audit" \
    -l since -d "Time range (1h, 24h, 7d)"
complete -c cognos -n "__fish_seen_subcommand_from audit" \
    -l agent -a "planner memory security scheduler file coding ui coordinator" \
    -d "Filter by agent"
complete -c cognos -n "__fish_seen_subcommand_from audit" \
    -l action -d "Filter by action type"

# predict subcommands
complete -c cognos -n "__fish_seen_subcommand_from predict" -f \
    -a "history disable enable status"
complete -c cognos -n "__fish_seen_subcommand_from predict" \
    -l scope -d "Domain to disable (coding, writing, research, personal)"

# model subcommands
complete -c cognos -n "__fish_seen_subcommand_from model" -f \
    -a "list pull remove info set"
complete -c cognos -n "__fish_seen_subcommand_from model" \
    -n "__fish_seen_subcommand_from pull set remove info" \
    -a "mistral-7b phi-3-mini codestral" -d "Model name"

# agent subcommands
complete -c cognos -n "__fish_seen_subcommand_from agent" -f \
    -a "status restart logs"
complete -c cognos -n "__fish_seen_subcommand_from agent" \
    -n "__fish_seen_subcommand_from restart logs" \
    -a "planner memory security scheduler file coding ui" \
    -d "Agent name"
complete -c cognos -n "__fish_seen_subcommand_from agent" \
    -l since -d "Time range for logs (1h, 24h, 7d)"

# cache subcommands
complete -c cognos -n "__fish_seen_subcommand_from cache" -f -a "clear"