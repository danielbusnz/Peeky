#!/bin/bash
# Installs git hooks for auto-updating the binary
set -e

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HOOKS_DIR="$REPO_ROOT/.git/hooks"

# Post-merge hook: runs after git pull/merge
cat > "$HOOKS_DIR/post-merge" << 'EOF'
#!/bin/bash
# Auto-rebuild peeky binary after pulling changes

# Check if peeky source files changed
CHANGED_FILES=$(git diff-tree -r --name-only --no-commit-id ORIG_HEAD HEAD)
if echo "$CHANGED_FILES" | grep -q "^peeky/"; then
    echo "Peeky source changed, rebuilding binary..."
    ./scripts/update-binary.sh
fi
EOF

chmod +x "$HOOKS_DIR/post-merge"
echo "Installed post-merge hook"

# Post-checkout hook: runs after git checkout/switch
cat > "$HOOKS_DIR/post-checkout" << 'EOF'
#!/bin/bash
# Auto-rebuild if switching branches with peeky changes
# Args: $1=prev HEAD, $2=new HEAD, $3=1 if branch checkout

if [ "$3" = "1" ]; then
    CHANGED_FILES=$(git diff --name-only "$1" "$2")
    if echo "$CHANGED_FILES" | grep -q "^peeky/"; then
        echo "Peeky source differs on this branch, rebuilding binary..."
        ./scripts/update-binary.sh
    fi
fi
EOF

chmod +x "$HOOKS_DIR/post-checkout"
echo "Installed post-checkout hook"

echo "Done! Hooks will auto-rebuild peeky binary when source changes."
