"""Runs a built binary once, so macOS assesses it before any test runs it.

macOS assesses every new executable file on its first run, and Bazel makes a
new file for a binary in each output base. This action runs the binary (the
same file every test in this output base runs, through runfiles links) and
records its SHA-256. It is never taken from any cache (no-cache): Bazel
applies no-remote-cache to the remote cache only, and served this action
from the disk cache, so a worktree whose binary came from that cache never
ran its own file. With no-cache it runs in every build that needs it; once
macOS has assessed the file that costs one `--version` and a checksum, and
the stamp's content is unchanged, so nothing that depends on it reruns.
"""

def _warm_binary_impl(ctx):
    bin = ctx.executable.binary
    stamp = ctx.actions.declare_file(ctx.label.name + ".stamp")
    ctx.actions.run_shell(
        inputs = [bin],
        outputs = [stamp],
        command = '"$1" --version >/dev/null && shasum -a 256 "$1" | cut -d" " -f1 > "$2"',
        arguments = [bin.path, stamp.path],
        mnemonic = "WarmBinary",
        progress_message = "Running %{input} once",
        execution_requirements = {"no-cache": "1"},
    )
    return [DefaultInfo(files = depset([stamp]), runfiles = ctx.runfiles(files = [stamp, bin]))]

warm_binary = rule(
    implementation = _warm_binary_impl,
    attrs = {"binary": attr.label(mandatory = True, executable = True, cfg = "target")},
)
