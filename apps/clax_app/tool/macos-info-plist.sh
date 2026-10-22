#!/usr/bin/env bash
# macos-info-plist.sh PLIST XCCONFIG OUT: writes the Runner's Info.plist
# PLIST to OUT with the values the Xcode build fills in and rules_apple does
# not: $(PRODUCT_COPYRIGHT) from XCCONFIG (macos/Runner/Configs/
# AppInfo.xcconfig), XML-escaped, and $(DEVELOPMENT_LANGUAGE) as en.
set -euo pipefail
plist="$1" xcconfig="$2" out="$3"
copyright="$(sed -n 's/^PRODUCT_COPYRIGHT[[:space:]]*=[[:space:]]*//p' "$xcconfig")"
[ -n "$copyright" ] || { echo "no PRODUCT_COPYRIGHT in $xcconfig" >&2; exit 1; }
COPYRIGHT="$copyright" awk '
  BEGIN {
    c = ENVIRON["COPYRIGHT"]
    gsub(/&/, "\\&amp;", c); gsub(/</, "\\&lt;", c); gsub(/>/, "\\&gt;", c)
  }
  {
    line = $0
    while ((i = index(line, "$(PRODUCT_COPYRIGHT)")) > 0)
      line = substr(line, 1, i - 1) c substr(line, i + length("$(PRODUCT_COPYRIGHT)"))
    while ((i = index(line, "$(DEVELOPMENT_LANGUAGE)")) > 0)
      line = substr(line, 1, i - 1) "en" substr(line, i + length("$(DEVELOPMENT_LANGUAGE)"))
    print line
  }' "$plist" > "$out"
