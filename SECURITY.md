# Security policy

## Supported versions

Only the latest release, [`Kiled-arm64.dmg`](https://github.com/timwuhaotian/kiled/raw/main/releases/Kiled-arm64.dmg)
and `main`, gets security fixes.

## Reporting a vulnerability

Please **don't open a public issue.** Report it privately through GitHub:
[**Report a vulnerability**](https://github.com/timwuhaotian/kiled/security/advisories/new).

Include what an attacker could do, the steps to reproduce it, and the macOS version and
Kiled build. You'll get a reply within a week. Once a fix is released, the advisory is
published with credit to you, unless you'd rather not be named.

## Scope

Kiled runs the language servers installed on your machine, with your privileges, on the
projects you open; what a server does is that server's responsibility. These are in scope,
for example:

- A file's contents (or a server's answer) that make Kiled run a program, write a file or
  leak data without the user asking.
- Saving that writes somewhere other than the file being saved, or leaves a file truncated.
- A release DMG or app whose signature or notarization doesn't hold.
