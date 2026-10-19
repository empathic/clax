#!/usr/bin/env bash
# macos-app-check.sh ZIP HOME DEBUG_ENTITLEMENTS: checks the Bazel-built macOS app (a zip of
# Clax.app) against what `flutter build macos` makes: its bundle ID, name,
# copyright, version and executable, the camera's and location's usage
# strings, the app menu's About, Hide and Quit
# items named Clax, the whole menu bar (MainMenu.xib's items, keys,
# actions and tags), Flutter's frameworks, the sandbox, camera and location
# entitlements of Runner/Release.entitlements, and clax in Contents/Resources signed as a
# sandboxed helper (Runner/ClaxHelper.entitlements), and the home bundle
# HOME (the one //apps/clax_app:home_bundle built) and the third-party
# notices in Flutter's assets; and that the debug build's entitlements
# (DEBUG_ENTITLEMENTS, Runner/DebugProfile.entitlements) allow the camera
# and location too.
set -euo pipefail
zip="$1" home="$2" debug_ents="$3"
fails=0
ok() { echo "PASS: $1"; }
bad() { echo "FAIL: $1"; fails=1; }
dir="$(mktemp -d "${TEST_TMPDIR:-${TMPDIR:-/tmp}}/macos-app.XXXXXX")"
trap 'rm -rf "$dir"' EXIT
unzip -q "$zip" -d "$dir"
app="$dir/Clax.app"
[ -d "$app" ] || { echo "FAIL: no Clax.app in $zip"; exit 1; }
# The third-party notices the licences page lists (NOTICES.Z, from
# //apps/clax_app:notices): more than 500 kB, with entries for pub packages,
# the engine's components and the embedded clax's Rust crates.
check_notices() { # check_notices NOTICES.Z
    local text="$1.txt" n
    if ! gunzip -c "$1" > "$text"; then bad "NOTICES.Z is gzip"; return; fi
    n=$(( $(wc -c < "$text") ))
    if [ "$n" -gt 500000 ]; then ok "NOTICES holds the third-party notices ($n bytes)"; else bad "NOTICES holds the third-party notices (only $n bytes)"; fi
    for p in webview_flutter archive flutter skia ring tokio rusqlite; do
        if grep -qxF "$p" "$text"; then ok "NOTICES names $p"; else bad "NOTICES names $p"; fi
    done
}
plist() { /usr/libexec/PlistBuddy -c "Print :$1" "$app/Contents/Info.plist" 2>/dev/null || true; }
if [ "$(plist CFBundleIdentifier)" = dev.empathic.clax ]; then ok "bundle ID"; else bad "bundle ID: $(plist CFBundleIdentifier)"; fi
if [ "$(plist CFBundleName)" = Clax ]; then ok "bundle name"; else bad "bundle name: $(plist CFBundleName)"; fi
copyright="Copyright © 2026 Empathic, Inc."
if [ "$(plist NSHumanReadableCopyright)" = "$copyright" ]; then ok "copyright"; else bad "copyright: $(plist NSHumanReadableCopyright)"; fi
if [ "$(plist CFBundleShortVersionString)" = "$VERSION_NAME" ]; then ok "version name"; else bad "version name: $(plist CFBundleShortVersionString)"; fi
if [ "$(plist CFBundleVersion)" = "$VERSION_CODE" ]; then ok "build number"; else bad "build number: $(plist CFBundleVersion)"; fi
if [ -x "$app/Contents/MacOS/$(plist CFBundleExecutable)" ]; then ok "executable"; else bad "executable"; fi
# The usage strings macOS shows when Clax first asks for the camera and
# location (Runner/Info.plist), in the iOS app's words.
for k in "NSCameraUsageDescription=An app you made in Clax can take a photo when you let it." \
    "NSLocationUsageDescription=An app you made in Clax can use your location when you let it." \
    "NSLocationWhenInUseUsageDescription=An app you made in Clax can use your location when you let it."; do
    if [ "$(plist "${k%%=*}")" = "${k#*=}" ]; then ok "usage string ${k%%=*}"; else bad "usage string ${k%%=*}: $(plist "${k%%=*}")"; fi
done
nib="$app/Contents/Resources/Base.lproj/MainMenu.nib"
[ -d "$nib" ] && nib="$nib/keyedobjects.nib"
for t in "About Clax" "Hide Clax" "Quit Clax" "showAbout:"; do
    if [ -f "$nib" ] && grep -aq "$t" "$nib"; then ok "the menu has $t"; else bad "the menu has $t"; fi
done
# The menu bar as the compiled nib holds it (an NIBArchive), one line per
# item: its menus, title, key equivalent ("-" for none), action and tag. Clax's own items
# send claxMenuAction: with the tag of their ClaxCommand
# (lib/ui/shortcuts.dart).
menu_bar() { # menu_bar NIB
    python3 -I - "$1" <<'PY'
import struct, sys

data = open(sys.argv[1], "rb").read()
if data[:10] != b"NIBArchive":
    sys.exit("not an NIBArchive")
(_, _, nobj, oobj, nkey, okey, nval, oval, ncls, ocls) = struct.unpack_from("<10I", data, 10)

def varint(p):
    r = s = 0
    while True:
        b = data[p]
        p += 1
        r |= (b & 0x7F) << s
        s += 7
        if b & 0x80:
            return r, p

keys, classes, values, objects = [], [], [], []
p = okey
for _ in range(nkey):
    n, p = varint(p)
    keys.append(data[p:p + n].decode())
    p += n
p = ocls
for _ in range(ncls):
    n, p = varint(p)
    extra, p = varint(p)
    p += 4 * extra
    classes.append(data[p:p + n].rstrip(b"\0").decode())
    p += n
sizes = {0: 1, 1: 2, 2: 4, 3: 8, 6: 4, 7: 8, 10: 4}
p = oval
for _ in range(nval):
    k, p = varint(p)
    t = data[p]
    p += 1
    if t in (0, 1, 2, 3):
        v = int.from_bytes(data[p:p + sizes[t]], "little", signed=True)
    elif t in (4, 5):
        v = t == 5
    elif t == 8:
        n, p = varint(p)
        v = data[p:p + n]
        p += n
    elif t == 10:
        v = ("obj", struct.unpack_from("<I", data, p)[0])
    else:
        v = None
    p += sizes.get(t, 0)
    values.append((keys[k], v))
p = oobj
for _ in range(nobj):
    c, p = varint(p)
    vi, p = varint(p)
    vc, p = varint(p)
    objects.append((classes[c], values[vi:vi + vc]))

def get(o, key, default=None):
    for k, v in objects[o][1]:
        if k == key:
            return v[1] if isinstance(v, tuple) else v
    return default

def text(o):
    return "" if o is None else get(o, "NS.bytes", b"").decode()

actions = {}
for c, vs in objects:
    if c == "NSNibControlConnector":
        d = dict((k, v) for k, v in vs)
        actions[d["NSSource"][1]] = text(d["NSLabel"][1])

def key_of(item):
    k = text(get(item, "NSKeyEquiv"))
    if not k:
        return ""
    mask = get(item, "NSKeyEquivModMask", 0)
    shift = mask & (1 << 17) or (k.isalpha() and k.isupper())
    mods = "".join(
        sym for sym, on in (
            ("⌃", mask & (1 << 18)), ("⌥", mask & (1 << 19)), ("⇧", shift), ("⌘", mask & (1 << 20)))
        if on)
    return mods + k.upper()

def walk(menu, path):
    for _, v in objects[get(menu, "NSMenuItems")][1]:
        if not isinstance(v, tuple):
            continue
        item = v[1]
        if get(item, "NSIsSeparator", False):
            print(path + "---")
            continue
        title = text(get(item, "NSTitle"))
        line = [path + title, key_of(item) or "-", actions.get(item, "")]
        tag = get(item, "NSTag", 0)
        if tag and line[2] == "claxMenuAction:":
            line[2] += " %d" % tag
        print(" | ".join(line) if line[2] else line[0])
        sub = get(item, "NSSubmenu")
        if sub is not None and path == "":
            walk(sub, title + " › ")
        elif sub is not None:
            walk(sub, path + title + " › ")

for i, (c, _) in enumerate(objects):
    if c == "NSMenu" and text(get(i, "NSName")) == "_NSMainMenu":
        walk(i, "")
PY
}
expected_menu_bar() {
    cat <<'MENU'
Clax
Clax › About Clax | - | showAbout:
Clax › ---
Clax › Settings… | ⌘, | claxMenuAction: 19
Clax › ---
Clax › Services
Clax › ---
Clax › Hide Clax | ⌘H | hide:
Clax › Hide Others | ⌥⌘H | hideOtherApplications:
Clax › Show All | - | unhideAllApplications:
Clax › ---
Clax › Quit Clax | ⌘Q | terminate:
File
File › New App… | ⌘N | claxMenuAction: 1
File › Import App… | ⌘O | claxMenuAction: 2
File › Share App… | - | claxMenuAction: 3
File › ---
File › Close Window | ⌘W | performClose:
Edit
Edit › Undo | ⌘Z | undo:
Edit › Redo | ⇧⌘Z | redo:
Edit › ---
Edit › Cut | ⌘X | cut:
Edit › Copy | ⌘C | copy:
Edit › Paste | ⌘V | paste:
Edit › Paste and Match Style | ⌥⇧⌘V | pasteAsPlainText:
Edit › Delete | - | delete:
Edit › Select All | ⌘A | selectAll:
Edit › ---
Edit › Find
Edit › Find › Find… | ⌘F | performFindPanelAction:
View
View › Show Sidebar | ⌃⌘S | claxMenuAction: 4
View › ---
View › Comment | ⇧⌘K | claxMenuAction: 5
View › Show Threads | ⌘K | claxMenuAction: 6
View › ---
View › Reload App | ⌘R | claxMenuAction: 7
View › ---
View › Actual Size | ⌘0 | claxMenuAction: 8
View › Zoom In | ⌘+ | claxMenuAction: 9
View › Zoom Out | ⌘- | claxMenuAction: 10
View › ---
View › Enter Full Screen | ⌃⌘F | toggleFullScreen:
App
App › Back | ⌘[ | claxMenuAction: 11
App › Forward | ⌘] | claxMenuAction: 12
App › Home | ⇧⌘H | claxMenuAction: 13
App › ---
App › Inspect | ⌥⌘I | claxMenuAction: 14
App › Versions… | - | claxMenuAction: 15
App › Star | - | claxMenuAction: 16
App › Resume Build | - | claxMenuAction: 17
Window
Window › Minimize | ⌘M | performMiniaturize:
Window › Zoom | - | performZoom:
Window › ---
Window › Inbox | ⇧⌘I | claxMenuAction: 18
Window › ---
Window › Bring All to Front | - | arrangeInFront:
Help
Help › Clax Help | - | showClaxHelp:
MENU
}
if [ -f "$nib" ] && menu_bar "$nib" > "$dir/menu.txt" && expected_menu_bar | diff -u - "$dir/menu.txt"; then
    ok "the menu bar: Clax, File, Edit, View, App, Window and Help, with their keys and actions"
else
    bad "the menu bar: Clax, File, Edit, View, App, Window and Help, with their keys and actions (diff above)"
fi
if [ -f "$nib" ] && ! grep -aq APP_NAME "$nib"; then ok "the menu names no APP_NAME"; else bad "the menu names no APP_NAME"; fi
for f in Frameworks/FlutterMacOS.framework/FlutterMacOS Frameworks/App.framework/App; do
    if [ -e "$app/Contents/$f" ]; then ok "$f"; else bad "$f"; fi
done
assets="$app/Contents/Frameworks/App.framework/Resources/flutter_assets/assets/home"
if cmp -s "$assets/home.clax" "$home"; then ok "the home bundle built from the sources"; else bad "the home bundle built from the sources ($assets/home.clax and $home)"; fi
if [ -f "$assets/release.json" ]; then ok "the home bundle's release record"; else bad "the home bundle's release record"; fi
cp "$app/Contents/Frameworks/App.framework/Resources/flutter_assets/NOTICES.Z" "$dir/NOTICES.Z"
check_notices "$dir/NOTICES.Z"
ents="$(/usr/bin/codesign -d --entitlements - --xml "$app" 2>/dev/null || true)"
for k in com.apple.security.app-sandbox com.apple.security.network.client com.apple.security.network.server \
    com.apple.security.device.camera com.apple.security.personal-information.location; do
    if grep -q "$k" <<<"$ents"; then ok "app entitlement $k"; else bad "app entitlement $k"; fi
done
for k in com.apple.security.device.camera com.apple.security.personal-information.location; do
    if [ "$(/usr/libexec/PlistBuddy -c "Print :$k" "$debug_ents" 2>/dev/null)" = true ]; then ok "debug entitlement $k"; else bad "debug entitlement $k"; fi
done
helper="$app/Contents/Resources/clax"
if [ -x "$helper" ]; then ok "clax in Resources"; else bad "clax in Resources"; fi
hents="$(/usr/bin/codesign -d --entitlements - --xml "$helper" 2>/dev/null || true)"
for k in com.apple.security.app-sandbox com.apple.security.inherit; do
    if grep -q "$k" <<<"$hents"; then ok "clax entitlement $k"; else bad "clax entitlement $k"; fi
done
if /usr/bin/codesign --verify --deep --strict "$app"; then ok "signature verifies"; else bad "signature verifies"; fi
exit "$fails"
