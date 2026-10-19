#!/usr/bin/env bash
# Checks scripts/check-ios-spawns.sh on small trees: a process launch in a
# crate the iOS daemon links is flagged unless scripts/ios-spawns.allow
# names its function, a stale entry is flagged, test code is not scanned,
# and a module the allow file skips must be compiled out on iOS.
set -euo pipefail
cd "$(dirname "$0")/.."
check="$PWD/scripts/check-ios-spawns.sh"
d="$(mktemp -d)"; trap 'rm -rf "$d"' EXIT
fails=0

# tree: a fresh minimal checkout under $d/t with the given allow file.
tree() {
    rm -rf "$d/t"
    mkdir -p "$d/t/scripts" "$d/t/crates/clax-core/src" "$d/t/crates/clax-mcp/src" "$d/t/crates/clax-cli/src"
    printf '%s\n' "$1" > "$d/t/scripts/ios-spawns.allow"
    cat > "$d/t/crates/clax-core/src/lib.rs" <<'EOF'
/// Runs ps.
pub fn started(pid: u32) -> Option<u64> {
    #[cfg(not(target_os = "ios"))]
    {
        let _ = std::process::Command::new("ps").arg(pid.to_string()).output();
    }
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn runs() {
        let _ = std::process::Command::new("true").status();
    }
}
EOF
    cat > "$d/t/crates/clax-core/src/x_tests.rs" <<'EOF'
fn helper() { let _ = std::process::Command::new("true").status(); }
EOF
    cat > "$d/t/crates/clax-mcp/src/lib.rs" <<'EOF'
#[cfg(not(any(target_arch = "wasm32", target_os = "ios")))]
pub mod shim;
pub mod tools;
EOF
    cat > "$d/t/crates/clax-mcp/src/shim.rs" <<'EOF'
pub fn lsof() { let _ = std::process::Command::new("lsof").status(); }
EOF
    echo 'pub fn none() {}' > "$d/t/crates/clax-mcp/src/tools.rs"
    # Not linked by the iOS daemon: never scanned.
    echo 'pub fn run() { let _ = std::process::Command::new("claude").status(); }' > "$d/t/crates/clax-cli/src/lib.rs"
}

expect() { # expect pass|fail NAME [MESSAGE]
    local out got
    if out="$("$check" "$d/t" 2>&1)"; then got=pass; else got=fail; fi
    if [ "$got" != "$1" ]; then
        echo "FAIL: $2: expected $1, got $got: $out"; fails=1
    elif [ -n "${3:-}" ] && ! printf '%s\n' "$out" | grep -F -- "$3" >/dev/null; then
        echo "FAIL: $2: no \"$3\" in: $out"; fails=1
    else
        echo "PASS: $2"
    fi
}

ALLOW='# A comment line.
crates/clax-core/src/lib.rs started  # the ps arm is off on iOS
skip crates/clax-mcp/src/shim.rs  # not compiled for iOS'

tree "$ALLOW"
expect pass "the tree passes"

tree "$ALLOW"
cat >> "$d/t/crates/clax-core/src/lib.rs" <<'EOF'

pub fn open(url: &str) {
    let _ = std::process::Command::new("open").arg(url).status();
}
EOF
expect fail "a launch in an unlisted function" "crates/clax-core/src/lib.rs:"

tree "$ALLOW"
echo 'pub async fn push() { let _ = tokio::process::Command::new("codex").status().await; }' >> "$d/t/crates/clax-mcp/src/tools.rs"
expect fail "a tokio launch in an unlisted function" "crates/clax-mcp/src/tools.rs:"

tree "$ALLOW"
echo 'pub fn f() { let _ = unsafe { libc::fork() }; }' >> "$d/t/crates/clax-mcp/src/tools.rs"
expect fail "a fork" "fork"

tree "$ALLOW"
echo '// Command::new("ps") in a comment' >> "$d/t/crates/clax-mcp/src/tools.rs"
expect pass "a launch in a comment"

tree "$ALLOW
crates/clax-core/src/lib.rs gone  # removed"
expect fail "a stale entry" "gone"

tree "$ALLOW"
printf 'pub mod shim;\npub mod tools;\n' > "$d/t/crates/clax-mcp/src/lib.rs"
expect fail "a skipped module compiled on iOS" "crates/clax-mcp/src/shim.rs"

tree "crates/clax-core/src/lib.rs started"
expect fail "an unskipped module's launch" "crates/clax-mcp/src/shim.rs:"

exit "$fails"
