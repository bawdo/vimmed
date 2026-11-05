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

## Verifying a release

Every release asset (the macOS app zip, the `.deb`, the `.rpm` and the
PKGBUILD tarball) carries a build provenance attestation from the release
workflow, and a `checksums.txt` lists the asset digests. To confirm a
download came from this repository's CI and has not been altered, run both
against the asset you downloaded:

```bash
gh attestation verify vimmed_<version>_macos_arm64.zip --repo bawdo/vimmed \
  --signer-workflow bawdo/vimmed/.github/workflows/release.yml \
  --source-ref refs/heads/main --deny-self-hosted-runners
shasum -a 256 -c checksums.txt --ignore-missing
```

`--repo` on its own only proves that some workflow in this repository, run
from any branch, signed the asset. The other three flags pin it to the
release workflow running from `main` on a GitHub-hosted runner, which rules
out an attestation that someone with push access makes from a throwaway
branch, from any other workflow file or on a self-hosted runner.

Each release also carries the attestation as a Sigstore bundle,
`vimmed_<version>_provenance.intoto.jsonl`. Download it next to the asset
to verify without querying GitHub's attestation API:

```bash
gh attestation verify vimmed_<version>_macos_arm64.zip --repo bawdo/vimmed \
  --signer-workflow bawdo/vimmed/.github/workflows/release.yml \
  --source-ref refs/heads/main --deny-self-hosted-runners \
  --bundle vimmed_<version>_provenance.intoto.jsonl
```

On a machine with no network access at all, `gh` can't fetch the Sigstore
trusted root itself. Save it on a connected machine, copy it across and
pass it in:

```bash
gh attestation trusted-root > trusted_root.jsonl
gh attestation verify vimmed_<version>_macos_arm64.zip --repo bawdo/vimmed \
  --signer-workflow bawdo/vimmed/.github/workflows/release.yml \
  --source-ref refs/heads/main --deny-self-hosted-runners \
  --bundle vimmed_<version>_provenance.intoto.jsonl \
  --custom-trusted-root trusted_root.jsonl
```

The macOS app is also signed with a Developer ID certificate and notarised
by Apple, with the ticket stapled to the bundle. Once unzipped, check that
Gatekeeper accepts it:

```bash
spctl --assess --type execute -vv Vimmed.app
```

It should print `accepted` and `source=Notarized Developer ID`.
