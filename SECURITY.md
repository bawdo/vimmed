# Security policy

## Scope

vimmed opens and writes local files, reads a config file and spawns its own
window process. Bugs that let a crafted Markdown file or config file execute
code, write outside the file that was opened, or crash in a way an attacker
controls are security bugs.

## Supported versions

Only the newest release on the
[Releases page](https://github.com/bawdo/vimmed/releases) receives
fixes. There are no maintenance branches.

## Reporting a vulnerability

Please do not open a public issue for a security problem.

Use GitHub's private reporting form:
<https://github.com/bawdo/vimmed/security/advisories/new>. It goes
straight to the maintainer and nobody else can see it.

Include what you can of: the file or config that triggers it, the version
from `vimmed --version`, what you expected, and what happened. A proof of
concept helps but is not required.

You will get an acknowledgement within seven days. A fix, or an explanation
of why the report is not being treated as a vulnerability, follows as a
published advisory and a release. Credit is given in the advisory unless
you ask otherwise.
