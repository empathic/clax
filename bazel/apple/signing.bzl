"""The iOS app's signing inputs on this machine: `@clax_apple_signing`.

Signing material is this machine's and not pinned, like the Android debug
key (bazel/android/debug_keystore.bzl) and the home release key
(bazel/flutter/home_release.bzl). It is read from one directory,
`~/.config/clax/apple/` (`CLAX_APPLE_DIR` names another):

    team-id                            the team's 10-character ID
    clax-development.mobileprovision   the development profile (device builds)
    clax-appstore.mobileprovision      the App Store profile (TestFlight)
    AuthKey_<KEYID>.p8                 the App Store Connect API key, with
    api-key-id, api-issuer-id          its key ID and issuer ID beside it

The repository exposes:

    :team_id                    a file holding the team ID
    :development, :appstore     the profiles
    :api_key                    the key and its two ID files
    :entitlements_development   application-identifier, the team, the
                                keychain group and get-task-allow
    :entitlements_appstore      the same without get-task-allow

The entitlements name the app as `<team>.$(CFBundleIdentifier)`, which
rules_apple fills in with the app's bundle ID, so the bundle ID stays one
constant (`IOS_BUNDLE_ID` in bazel/flutter/ios.bzl).

A target whose inputs are incomplete fails when it is used, naming every
file that use needs and lacks, the directory, and docs/usage.md's section
"Signing for a device and TestFlight", which says how to get each one: a
development build needs the team ID and the development profile; the
TestFlight build needs the team ID, the App Store profile and the API key.
A directory or file that the group or others may access refuses every
target, naming its path and mode. Simulator builds depend on none of this,
so they build without the directory.

The repository is `local` and watches the directory and every file in it,
so it is evaluated again when a file is added, removed, changed or has its
mode changed. Bazel does not see the directory's own mode change: it is
read again when the server starts, or at `bazelisk fetch --force
--repo=@clax_apple_signing`, which the refusal names. The actions that read these files (the `ipa_*` apps of
bazel/flutter/ios.bzl) are tagged `no-remote` and `no-cache`, so neither a
profile nor a key enters a cache.
"""

_DOC = "docs/usage.md, \"Signing for a device and TestFlight\""

_KEY = "AuthKey_<KEYID>.p8"

# What each file is, as a missing one is named.
_WHAT = {
    "team-id": "the team's 10-character ID",
    "clax-development.mobileprovision": "the development provisioning profile",
    "clax-appstore.mobileprovision": "the App Store provisioning profile",
    _KEY: "the App Store Connect API key",
    "api-key-id": "the API key's key ID",
    "api-issuer-id": "the API key's issuer ID",
}

_DEVELOPMENT = ["team-id", "clax-development.mobileprovision"]
_APPSTORE = ["team-id", "clax-appstore.mobileprovision", _KEY, "api-key-id", "api-issuer-id"]
_API_KEY = [_KEY, "api-key-id", "api-issuer-id"]

# Each target: what it is for, the inputs that use needs, and the files
# (in this repository) it is made of.
_TARGETS = {
    "team_id": ("the team ID", ["team-id"], ["team-id"]),
    "development": ("a development device build", _DEVELOPMENT, ["clax-development.mobileprovision"]),
    "entitlements_development": ("a development device build", _DEVELOPMENT, ["entitlements_development.plist"]),
    "appstore": ("the TestFlight build", _APPSTORE, ["clax-appstore.mobileprovision"]),
    "entitlements_appstore": ("the TestFlight build", _APPSTORE, ["entitlements_appstore.plist"]),
    "api_key": ("the TestFlight upload", _API_KEY, ["api-key-id", "api-issuer-id"]),
}

def _mode(ctx, path):
    """The permission bits of `path` as an int, read with stat(1)."""
    if "mac" in ctx.os.name:
        args = ["stat", "-f", "%Lp", str(path)]
    else:
        args = ["stat", "-c", "%a", str(path)]
    res = ctx.execute(args)
    if res.return_code != 0:
        fail("bazel/apple/signing.bzl: cannot read the mode of %s:\n%s" % (path, res.stderr))
    return int(res.stdout.strip(), 8)

def _octal(mode):
    digits = ""
    for _ in range(4):
        digits = "01234567"[mode % 8] + digits
        mode //= 8
    return digits

def _read_id(ctx, path):
    return ctx.read(path).strip() if path.exists else ""

def _is_team_id(s):
    return len(s) == 10 and s.isalnum() and s.upper() == s

def _plist(team, get_task_allow):
    app = "%s.$(CFBundleIdentifier)" % team
    debug = "\t<key>get-task-allow</key>\n\t<true/>\n" if get_task_allow else ""
    return """\
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
\t<key>application-identifier</key>
\t<string>{app}</string>
\t<key>com.apple.developer.team-identifier</key>
\t<string>{team}</string>
{debug}\t<key>keychain-access-groups</key>
\t<array>
\t\t<string>{app}</string>
\t</array>
</dict>
</plist>
""".format(app = app, team = team, debug = debug)

def _impl(ctx):
    home = ctx.getenv("HOME", "")
    apple = ctx.path(ctx.getenv("CLAX_APPLE_DIR", "") or "%s/.config/clax/apple" % home)

    # Watching the directory brings the repository up to date when a file
    # appears or goes; watching each file, when one changes or is chmodded.
    # Nothing brings it up to date when the directory alone is chmodded.
    ctx.watch(apple)
    refused = []
    present = {}
    if apple.exists and apple.is_dir:
        ctx.watch_tree(apple)
        mode = _mode(ctx, apple)
        if mode & 0o077:
            refused.append(
                "%s is mode %s: the group and others must have no access " % (apple, _octal(mode)) +
                "(chmod 700 %s, then bazelisk fetch --force --repo=@clax_apple_signing)" % apple,
            )
        for entry in apple.readdir():
            ctx.watch(entry)
            mode = _mode(ctx, entry)
            if mode & 0o077:
                refused.append("%s is mode %s: the group and others must have no access (chmod 600 %s)" % (entry, _octal(mode), entry))
            present[entry.basename] = entry

    # Each input: its file here, or why it is missing.
    missing = {}
    team = _read_id(ctx, apple.get_child("team-id"))
    if "team-id" not in present:
        missing["team-id"] = "missing"
    elif not _is_team_id(team):
        missing["team-id"] = "holds %r, not a 10-character team ID" % team
    else:
        ctx.file("team-id", team + "\n")
        ctx.file("entitlements_development.plist", _plist(team, True))
        ctx.file("entitlements_appstore.plist", _plist(team, False))
    for name in ["clax-development.mobileprovision", "clax-appstore.mobileprovision", "api-key-id", "api-issuer-id"]:
        if name in present:
            ctx.symlink(present[name], name)
        else:
            missing[name] = "missing"
    key_id = _read_id(ctx, apple.get_child("api-key-id"))
    key = "AuthKey_%s.p8" % key_id
    if not key_id:
        missing[_KEY] = "missing (its name comes from api-key-id)"
    elif key not in present:
        others = sorted([n for n in present if n.startswith("AuthKey_") and n.endswith(".p8")])
        missing[_KEY] = "missing: api-key-id names %s" % key
        if others:
            missing[_KEY] += ", and the directory holds %s" % ", ".join(others)
    else:
        ctx.symlink(present[key], key)

    targets = []
    for name, (use, needs, files) in _TARGETS.items():
        lacking = [n for n in needs if n in missing]
        if not lacking and not refused:
            srcs = files + ([key] if name == "api_key" else [])
            targets.append("""\
filegroup(
    name = "{name}",
    srcs = {srcs},
    visibility = ["//visibility:public"],
)
""".format(name = name, srcs = repr(srcs)))
            continue
        lines = ["The Apple signing inputs for %s are not ready, in %s:" % (use, apple)]
        if not apple.exists:
            lines.append("  the directory does not exist: create it with mode 0700 (mkdir -m 700 %s)" % apple)
        lines += ["  %s (%s): %s" % (n, _WHAT[n], missing[n]) for n in lacking]
        lines += ["  " + r for r in refused]
        lines.append("Each file, and how to get it: %s." % _DOC)
        targets.append("""\
signing_missing(
    name = "{name}",
    message = {message},
    visibility = ["//visibility:public"],
)
""".format(name = name, message = repr("\n".join(lines))))

    ctx.file("missing.bzl", """\
def _impl(ctx):
    fail("\\n" + ctx.attr.message)

signing_missing = rule(
    implementation = _impl,
    attrs = {"message": attr.string()},
    doc = "Fails when used, with the message of what the signing inputs lack.",
)
""")
    ctx.file("BUILD.bazel", "load(\":missing.bzl\", \"signing_missing\")\n\n" + "\n".join(targets))

_signing = repository_rule(
    implementation = _impl,
    environ = ["CLAX_APPLE_DIR", "HOME"],
    local = True,
)

def _ext_impl(_ctx):
    _signing(name = "clax_apple_signing")

signing = module_extension(implementation = _ext_impl)
