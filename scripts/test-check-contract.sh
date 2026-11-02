#!/usr/bin/env bash
# Checks scripts/check-contract.py on changes made to a copy of the
# committed contract (web/sdk/openapi.json): the contract against itself
# passes; a field gone from an answer, a field newly required in a request,
# an enum value an answer may now hold (a plain enum, or one of documented
# variants), an operation gone, retry turned off, and a value a request no
# longer takes (behind a nullable union too) each fail with the same major
# and pass with the next one; an optional field added to a request and a new
# operation pass with the same major. Bad usage exits 2.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
d="$(mktemp -d)"
trap 'rm -rf "$d"' EXIT
fails=0
_fail() { echo "FAIL: $1" >&2; fails=$((fails + 1)); }

# _variant NAME PYTHON [VERSION]: $d/NAME.json, the contract changed by the
# Python statements PYTHON (on `c`, the parsed contract, and `s`, its
# schemas), at VERSION when given.
_variant() {
    python3 - "$d/$1.json" "$2" "${3:-}" <<'PY'
import json, sys
out, change, version = sys.argv[1:]
c = json.load(open("web/sdk/openapi.json"))
s = c["components"]["schemas"]
exec(change)
if version:
    c["info"]["version"] = version
json.dump(c, open(out, "w"))
PY
}

# _expect pass|fail NAME [TEXT]: the check from the contract to $d/NAME.json;
# TEXT must be in what a failure prints.
_expect() {
    local out st
    out="$(python3 scripts/check-contract.py --old web/sdk/openapi.json --new "$d/$2.json" 2>&1)"
    st=$?
    case "$1" in
        pass) [ "$st" = 0 ] || _fail "$2 should pass: $out" ;;
        fail) { [ "$st" = 1 ] && grep -qF -- "$3" <<<"$out"; } || _fail "$2 should fail naming '$3': $out" ;;
    esac
}

major="$(python3 -c 'import json; print(json.load(open("web/sdk/openapi.json"))["info"]["version"].split(".")[0])')"
next="$((major + 1)).0.0"

_variant same ''
_expect pass same

# Breaks: each fails at the same major and passes at the next.
breaks=(
    'answer_field|del s["Badge"]["properties"]["waiting"]; s["Badge"]["required"].remove("waiting")|Badge|.waiting is gone'
    'request_required|s["CommentRequest"]["properties"]["kind"] = {"type": "string"}; s["CommentRequest"]["required"].append("kind")|required|.kind is now required'
    'answer_enum|s["ReportState"]["enum"].append("lost")|enum|may now hold "lost"'
    'answer_documented_enum|s["ReportersSee"]["oneOf"].append({"const": "some", "type": "string"})|oneOf|may now hold "some"'
    'request_nullable|s["ReportContextInput"]["properties"]["v"]["const"] = 2|nullable|no longer takes 1'
    'operation_gone|del c["paths"]["/api/queues/{qid}/reporter/badge"]|op|GET /api/queues/{qid}/reporter/badge is gone'
    'retry_off|c["paths"]["/api/queues/{qid}/reporter/badge"]["get"]["x-clax-retry"] = False|retry|may no longer be retried'
    'request_enum|s["CaptureMode"]["enum"].remove("area")|enum|no longer takes "area"'
    'answer_optional|s["Badge"]["required"].remove("waiting")|opt|.waiting is no longer always present'
    'answer_type|s["Badge"]["properties"]["waiting"]["type"] = "string"|type|may now hold string'
)
for b in "${breaks[@]}"; do
    IFS='|' read -r name change _ text <<<"$b"
    _variant "$name" "$change"
    _expect fail "$name" "$text"
    _variant "$name.major" "$change" "$next"
    _expect pass "$name.major"
done

# No break: the same major passes.
_variant request_optional 's["CommentRequest"]["properties"]["kind"] = {"type": "string"}'
_expect pass request_optional
_variant new_operation 'c["paths"]["/api/new"] = {"get": {"operationId": "new", "responses": {"200": {"description": "OK"}}}}'
_expect pass new_operation
_variant answer_field_added 's["Badge"]["properties"]["more"] = {"type": "string"}; s["Badge"]["required"].append("more")'
_expect pass answer_field_added

# A lower major fails whatever changed.
_variant lower '' "0.9.0"
python3 scripts/check-contract.py --old web/sdk/openapi.json --new "$d/lower.json" >/dev/null 2>&1 \
    && _fail "a lower major should fail"

python3 scripts/check-contract.py --old web/sdk/openapi.json >/dev/null 2>&1
[ $? = 2 ] || _fail "--old without --new should exit 2"

[ "$fails" = 0 ] || { echo "$fails failure(s)" >&2; exit 1; }
echo "check-contract: ok"
