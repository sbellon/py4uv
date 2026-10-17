# SPDX-FileCopyrightText: 2026 Stefan Bellon
#
# SPDX-License-Identifier: MIT

# Prints the CHANGELOG.md section of version v as GitHub release notes:
# without its heading, with each wrapped list item joined into one line
# (GitHub keeps line breaks in release notes) and no blank lines around it.
#
#   awk -v v=0.1.1 -f .github/release-notes.awk CHANGELOG.md

function flush() {
    if (!have) return
    if (line == "") { blanks++; return }
    if (printed) for (; blanks > 0; blanks--) print ""
    blanks = 0; print line; printed = 1
}
/^## / { if (p) exit; p = index($0, "## " v " ") == 1; found = found || p; next }
!p { next }
/^  +[^ ]/ { sub(/^ +/, " "); line = line $0; next }
{ flush(); line = $0; have = 1 }
END {
    flush()
    if (!found) { print "release-notes.awk: no section for version " v > "/dev/stderr"; exit 1 }
}
