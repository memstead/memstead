#!/usr/bin/env bash
# Replay of the four due rules of the 2026-11-10 probe on the packages of
# this run, where `due` reads its date from a recorded check
# (`check_kind`) instead of a field typed by hand.
#
# Usage: replay.sh <memstead binary> <empty scratch directory>
#
# Per rule: a fresh folder workspace with the package installed, the
# entity the rule names written by `author-a`, an independent check of
# the rule's kind by `checker-c` dated 2026-01-10, then `memstead due`
# pinned to 2027-06-01. A second check by `checker-d` dated 2026-06-01
# must move the date; the entity before any check reads due now.
set -euo pipefail

bin=$1
scratch=$2
here=$(cd "$(dirname "$0")" && pwd)
today=2027-06-01

m() { (cd "$ws" && "$bin" "$@" --quiet); }

setup() { # <package dir> <schema ref>
    ws="$scratch/$2"
    mkdir -p "$ws"
    m init --name probe --schema default@1.3.0 >/dev/null
    m schema install "$here/$1" >/dev/null
    m mem set-schema probe "$3" >/dev/null
}

check() { # <id> <kind> <identity> <observed_at>
    m check "$1" --kind "$2" --verdict ok --method "replay" \
        --identity "$3" --role checker --observed-at "$4" >/dev/null
}

line() { # <id>
    m due --within 3y --today "$today" | grep -F "\`$1\`" || echo "(not listed)"
}

rule() { # <label> <id> <kind>
    echo "## $1"
    echo "before any check: $(line "$2")"
    check "$2" "$3" checker-c 2026-01-10T12:00:00Z
    echo "after a check dated 2026-01-10: $(line "$2")"
    check "$2" "$3" checker-d 2026-06-01T09:00:00Z
    echo "after a later check dated 2026-06-01: $(line "$2")"
    echo
}

# Stock rule 1: the next due date of a yearly duty follows from the last
# recorded completion.
setup stockadmin stock-1 stockadmin@0.1.0
m create --type duty --title "Boiler service" --section "duty=yearly service" \
    --metadata status=active --metadata last_completed=2020-01-01 \
    --identity author-a --role author >/dev/null
rule "stock rule 1 (x-completion, 365 days)" probe--boiler-service x-completion

# Stock rule 9: a standing fact is confirmed again every 12 months,
# counted from the last recorded confirmation.
setup stockadmin stock-9 stockadmin@0.1.0
m create --type party --title "Gas supplier" --section "contact=hotline" \
    --metadata standing=current --metadata contact_confirmed_on=2020-01-01 \
    --identity author-a --role author >/dev/null
rule "stock rule 9 (x-confirmation, 365 days)" probe--gas-supplier x-confirmation

# Lab rule 2: a procedure is reviewed again every 24 months, counted from
# the last recorded review. The procedure becomes effective on an
# independent review first (lab rule 1), so the first review is that one.
setup labqm lab-2 labqm@0.1.0
m create --type procedure --title "Titration" --section "purpose=measure acidity" \
    --metadata status=draft --metadata version=1 \
    --identity author-a --role author >/dev/null
echo "## lab rule 2 (x-review, 730 days)"
check probe--titration x-review checker-c 2026-01-10T12:00:00Z
m update probe--titration --metadata status=effective --auto-hash \
    --identity author-a --role author >/dev/null
echo "effective on a review dated 2026-01-10: $(line probe--titration)"
check probe--titration x-review checker-d 2026-06-01T09:00:00Z
echo "after a later review dated 2026-06-01: $(line probe--titration)"
echo

# Inspection rule 2: an approval lapses 24 months after the inspection,
# the date coming from the recorded inspection.
setup inspection-body inspection-2 inspection-body@0.1.0
m create --type catalogue_edition --title "Catalogue 2026" --section "criteria=list" \
    --metadata status=current --identity author-a --role author >/dev/null
m create --type inspector --title "Inspector One" --section "profile=certified" \
    --metadata authorisation=authorised --identity author-a --role author >/dev/null
m create --type inspection --title "Inspection 1" --section "findings=none" \
    --metadata result=passed --identity author-a --role author >/dev/null
m create --type item --title "Lift" --section "technical_description=a lift" \
    --metadata status=draft --metadata class=a \
    --relation RESTS_ON:probe--inspection-1 --relation INSPECTED_AGAINST:probe--catalogue-2026 \
    --identity author-a --role author >/dev/null
echo "## inspection rule 2 (x-inspection, 730 days)"
check probe--lift x-inspection checker-c 2026-01-10T12:00:00Z
m update probe--lift --metadata status=approved --auto-hash \
    --identity author-a --role author >/dev/null
echo "approved on an inspection dated 2026-01-10: $(line probe--lift)"
check probe--lift x-inspection checker-d 2026-06-01T09:00:00Z
echo "after a later inspection dated 2026-06-01: $(line probe--lift)"
