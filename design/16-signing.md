# 16 — Signing the builds

**Not implemented.** This document is what it would take, verified against current
documentation on 2026-08-01, so the work can start without re-deriving it.

Signing is the highest-value distribution work remaining. It is not about tidiness: on a locked-down
corporate Windows workstation an unsigned binary is not merely warned about, it is **unrunnable**,
and no amount of packaging works around that.

---

## Attestation is not signing, and tropism already has one of them

Worth separating first, because they are easy to conflate and tropism ships one already.

| | What it proves | Who checks it | Status |
| --- | --- | --- | --- |
| **Build provenance attestation** | this artifact came from *that* commit, built by *that* workflow | a human, deliberately, with `gh attestation verify` | **shipping** — `github-attestations = true` |
| **Code signature** | a named publisher vouches for this binary | the operating system, automatically, before it will run | **absent** |

The attestation is real and verifiable today:

```
$ gh attestation verify tropism-aarch64-apple-darwin.tar.xz --owner grahambrooks
repo:     https://github.com/grahambrooks/tropism
commit:   3daca1db328ba0b9243e4d0a1a014d2064dca544
workflow: .../release.yml@refs/tags/v2026.8.4
```

That is excellent supply-chain hygiene and it does **nothing** for SmartScreen, Gatekeeper, or
AppLocker. Those gates ask a different question — "who signed this?" — and provenance does not answer
it. Do not let the presence of one imply the other.

---

## What signing actually buys, in order of importance

**1. AppLocker and WDAC publisher rules.** This is the big one, and it is the one usually missed.
Many enterprises block execution from user-writable directories outright, which defeats *every*
no-admin install location including the one dist's installer uses. A signed binary can be
allowlisted **by publisher** rather than by path — one policy entry that keeps working across every
future version. Unsigned, each release needs a new path exception, which in practice means none are
granted. Signing is what converts "blocked forever" into "one ticket, once".

**2. SmartScreen.** An unsigned `.exe` downloaded through a browser carries the Mark of the Web and
trips a reputation warning. Note the installer path partly sidesteps this already: `irm | iex` does
not set MOTW, so the PowerShell one-liner is quieter than a browser download today. Signing removes
the warning for people who take the archive instead.

**3. macOS Gatekeeper.** Same shape: `curl | sh` does not set `com.apple.quarantine`, so the shell
installer works unsigned. Downloading the `.tar.xz` in a browser does quarantine it, and an unsigned,
un-notarized binary is then refused.

The pattern across all three: **the installers are the quiet path and the archives are the loud one.**
Signing matters most for users who cannot or will not pipe a URL into a shell — which, in a regulated
environment, is most of them.

---

## Windows

### Option A — SSL.com eSigner, which dist supports natively

The only signing method dist implements. One config key:

```toml
[dist]
ssldotcom-windows-sign = "prod"   # or "test" against sandbox.ssl.com first
```

and four repository secrets:

| Secret | From |
| --- | --- |
| `SSLDOTCOM_USERNAME` | SSL.com account |
| `SSLDOTCOM_PASSWORD` | SSL.com account |
| `SSLDOTCOM_TOTP_SECRET` | the TOTP secret shown under the QR code when ordering — save it then, it is not shown again |
| `SSLDOTCOM_CREDENTIAL_ID` | Dashboard → developer tools → signing credentials |

Order **eSigner EV Code Signing** from SSL.com's developer tools dashboard. EV matters: it earns
SmartScreen reputation immediately rather than accumulating it over downloads.

**Cost:** several hundred USD per year. **Effort:** one config key. It signs the `.exe` inside the
archive, which is the part that has to be signed.

### Option B — SignPath Foundation, free for open source, but not a drop-in

[SignPath Foundation](https://signpath.io/solutions/open-source-community) signs qualifying OSS
projects at no cost, and tropism qualifies: OSI-approved licence (MIT, with the text in `LICENSE`
since D26 was resolved — the eligibility test is the text, not the `Cargo.toml` field), public repo, no
proprietary components, actively maintained and already releasing. Applications take days to weeks,
which is the argument for starting it before it is needed.

**The friction is dist.** SignPath signs via a GitHub Action that uploads an artifact and returns a
signed one. dist's `build-local-artifacts` job compiles *and packages* in one step, so by the time
any custom job runs, the `.exe` is already inside a `.zip` — and signing the archive does not satisfy
SmartScreen, which inspects the executable. dist's extension points
(`local-artifacts-jobs`, `global-artifacts-jobs`, `publish-jobs`, `post-announce-jobs`, referenced as
`"./job-name"`) run *around* its phases, not inside them.

**Unverified**, and the first thing to test if this route is taken: whether a
`local-artifacts-jobs` hook can sign before packaging, or whether it needs a separate workflow that
downloads the published release, unpacks, signs, repacks, and re-uploads — which works but breaks the
checksums and attestation dist generated, and would have to regenerate both.

### Recommendation for Windows

**Apply to SignPath Foundation now, and evaluate the integration while the application is
processed.** The lead time is the reason to start, and the answer to the integration question does
not change the application. If the integration proves genuinely awkward and the project has a budget,
SSL.com is one config key and dist does the rest.

---

## macOS

**dist does not implement macOS signing or notarization** — its documentation covers Windows only,
and [axodotdev/cargo-dist#1121](https://github.com/axodotdev/cargo-dist/issues/1121) tracks the
request. Doing it means a custom job, or signing outside dist entirely.

What it requires regardless of tooling:

- **Apple Developer Program**, $99/year. There is no free tier and no OSS programme.
- A **Developer ID Application** certificate, `codesign --timestamp --options runtime`, then
  `notarytool submit --wait`, then `stapler staple` so the ticket travels with the artifact offline.
- Both architectures signed, and the notarization submitted per artifact.

**Lower priority than Windows.** The macOS audience for a polyglot dependency analyzer skews toward
developers who already run `curl | sh`, and that path does not set the quarantine attribute. The
corporate lockdown problem that motivated this work is a Windows problem.

---

## Linux

Nothing to do. There is no per-binary signing convention that any Linux desktop or CI enforces;
distribution happens through package managers with their own repository signing, or through checksums.
tropism already ships `sha256.sum` and provenance attestation, which is the norm and is sufficient.

---

## Order of work

1. **Apply to SignPath Foundation.** Free, tropism qualifies, and the lead time dominates.
2. **Test whether a dist `local-artifacts-jobs` hook can sign before packaging.** This is the one
   unknown, and it decides whether route B is a config change or a second workflow.
3. **Sign Windows.** The platform where signing is the difference between running and not running.
4. **Revisit macOS** only if evidence appears that anyone is blocked by Gatekeeper.

Keep the attestation whichever way this goes. It answers a question signing does not, and it costs
one config key.
