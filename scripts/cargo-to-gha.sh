#!/bin/bash
# Convert cargo/clippy JSON output to GitHub Actions annotation format
# Usage: cargo clippy --message-format=json 2>&1 | cargo-to-gha.sh

# Read stdin and process each JSON line
while IFS= read -r line; do
    # Skip non-JSON lines
    if [[ ! "$line" =~ ^\{ ]]; then
        continue
    fi

    # Parse with jq - only process compiler-message with rendered output
    echo "$line" | jq -r '
        select(.reason == "compiler-message") |
        .message |
        select(.level == "error" or .level == "warning") |
        . as $msg |
        (.spans // [])[] |
        select(.is_primary == true) |
        "::"+$msg.level+" file="+.file_name+",line="+(.line_start|tostring)+",endLine="+(.line_end|tostring)+",col="+(.column_start|tostring)+",endColumn="+(.column_end|tostring)+",title="+($msg.code.code // "unknown")+"::"+$msg.message
    ' 2>/dev/null
done
