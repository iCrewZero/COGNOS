# COGNOS CLI — fish shell completions
# v0: stub — covers main subcommands

complete -c cognos -f

# Top-level subcommands
complete -c cognos -n "__fish_use_subcommand" -a intent -d "Submit an intent"
complete -c cognos -n "__fish_use_subcommand" -a approval -d "Manage pending approvals"
complete -c cognos -n "__fish_use_subcommand" -a memory -d "Query and manage memories"
complete -c cognos -n "__fish_use_subcommand" -a status -d "Show system status"
complete -c cognos -n "__fish_use_subcommand" -a tui -d "Launch the interactive TUI"
complete -c cognos -n "__fish_use_subcommand" -a version -d "Print version information"
complete -c cognos -n "__fish_use_subcommand" -a help -d "Show help"

# intent flags
complete -c cognos -n "__fish_seen_subcommand_from intent" -l dry-run -d "Show what would be done without executing"
complete -c cognos -n "__fish_seen_subcommand_from intent" -s p -l priority -d "Set intent priority" -a "low normal high critical"

# approval flags
complete -c cognos -n "__fish_seen_subcommand_from approval" -l list -d "List pending approvals"
complete -c cognos -n "__fish_seen_subcommand_from approval" -l approve -d "Approve by ID" -a "(__cognos_pending_approval_ids)"
complete -c cognos -n "__fish_seen_subcommand_from approval" -l deny -d "Deny by ID"
complete -c cognos -n "__fish_seen_subcommand_from approval" -l json -d "Output as JSON"

# memory subcommands
complete -c cognos -n "__fish_seen_subcommand_from memory" -a search -d "Search memories"
complete -c cognos -n "__fish_seen_subcommand_from memory" -a list -d "List all memories"
complete -c cognos -n "__fish_seen_subcommand_from memory" -a forget -d "Forget a memory"
complete -c cognos -n "__fish_seen_subcommand_from memory" -a edit -d "Edit a memory"

# status flags
complete -c cognos -n "__fish_seen_subcommand_from status" -l watch -d "Continuously update"
complete -c cognos -n "__fish_seen_subcommand_from status" -l agent -d "Filter by agent"

# global flags
complete -c cognos -s v -l verbose -d "Increase verbosity"
complete -c cognos -l config -d "Path to config file" -r

# Helper: enumerate pending approval IDs (requires jq)
if type -q jq
    function __cognos_pending_approval_ids
        cognos approval --list --json 2>/dev/null | jq -r '.[].id' 2>/dev/null
    end
end

# v0: stub — completions cover the main surface area
