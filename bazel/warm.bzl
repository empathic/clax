"""Runs a built binary once, so macOS assesses it before any test runs it.

macOS assesses every new executable file on its first run, and Bazel makes a
new file for a binary in each output base. This action runs the binary (the
same file every test in this output base runs, through runfiles links) and
records its SHA-256. It is never taken from the disk cache (no-remote-cache),
so a worktree whose binary came from the cache still runs its own file once;
the output base's action cache skips it while the binary is unchanged.
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
        execution_requirements = {"no-remote-cache": "1"},
    )
    return [DefaultInfo(files = depset([stamp]), runfiles = ctx.runfiles(files = [stamp, bin]))]

warm_binary = rule(
    implementation = _warm_binary_impl,
    attrs = {"binary": attr.label(mandatory = True, executable = True, cfg = "target")},
)
