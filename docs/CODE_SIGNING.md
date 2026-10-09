# Code Signing

> **TL;DR** — macOS Gatekeeper and Windows SmartScreen warnings cannot be
> removed in code. They disappear only when a release is **signed with a
> certificate** issued by Apple / a trusted CA. CI in this repository is
> already wired to sign automatically once the required secrets exist.

## Why the warnings appear

| Platform | Warning | Cause |
|---|---|---|
| macOS | *"MieoSQL is damaged / cannot be opened because the developer cannot be verified"* | App downloaded from the internet (quarantine flag) without a valid Apple signature + notarization |
| Windows | *"Windows protected your PC"* (SmartScreen) | The file/installer has no Authenticode signature, or the signer has no reputation yet |

Both systems let users continue anyway (see [Running unsigned builds](#running-unsigned-builds)),
but removing the warning requires signing.

---

## macOS — sign + notarize

### Requirements

- **Apple Developer Program — USD 99/year.** A free account can sign for local
  testing but **cannot notarize**, so downloaded builds still warn.
- A Mac to create the CSR/certificate.
- A **Developer ID Application** certificate (distribution outside the App Store).

### Create the certificate (one-time)

1. Create a CSR: Keychain Access → *Certificate Assistant → Request a
   Certificate From a Certificate Authority* (save the `.certSigningRequest`).
2. developer.apple.com → *Certificates, IDs & Profiles* → **+** → **Developer
   ID Application** → upload the CSR → download the `.cer`.
3. Open the `.cer` to install it in the login keychain, then export it as a
   `.p12`: Keychain Access → *My Certificates* → right-click the certificate →
   **Export** → set a password.
4. Base64-encode it for CI:
   ```bash
   openssl base64 -A -in certificate.p12 -out certificate-base64.txt
   ```
5. Create an **app-specific password** for notarization: appleid.apple.com →
   *Sign-In and Security → App-Specific Passwords* (do not use your real Apple
   ID password).

### CI secrets (GitHub → Settings → Secrets and variables → Actions)

| Secret | Value |
|---|---|
| `APPLE_CERTIFICATE` | Contents of `certificate-base64.txt` |
| `APPLE_CERTIFICATE_PASSWORD` | Password chosen when exporting the `.p12` |
| `APPLE_SIGNING_IDENTITY` | `Developer ID Application: Company Name (TEAMID)` — list with `security find-identity -v -p codesigning` |
| `APPLE_ID` | Apple ID email |
| `APPLE_PASSWORD` | App-specific password from step 5 |
| `APPLE_TEAM_ID` | Team ID (10 characters, from developer.apple.com) |

`APPLE_SIGNING_IDENTITY` is optional — Tauri finds the imported certificate
automatically.

> Instead of `APPLE_ID`/`APPLE_PASSWORD` you can use an App Store Connect API
> key: `APPLE_API_ISSUER`, `APPLE_API_KEY`, `APPLE_API_KEY_PATH` (`.p8` file).

### What CI does

When `APPLE_CERTIFICATE` is present, the `macos-arm64` job exports the
variables above and runs `cargo tauri build --bundles dmg`. Tauri imports the
certificate into a temporary keychain, signs the app with the hardened
runtime, **notarizes it with Apple, and staples the ticket**. Without secrets
the job builds unsigned, exactly as before.

### Verify

```bash
codesign -dv --verbose=4 /Applications/MieoSQL.app   # "Developer ID Application: ..."
spctl -a -vv /Applications/MieoSQL.app               # "accepted, source=Notarized Developer ID"
xcrun stapler validate /Applications/MieoSQL.app     # "The validate action worked!"
```

---

## Windows — Authenticode

### Options

| Option | Cost | Notes |
|---|---|---|
| **Azure Artifact Signing** (formerly Azure Trusted Signing) | ~USD 10/month | Cheapest, CI-friendly, no hardware token |
| **OV certificate** (DigiCert, SSL.com, Sectigo, Certum…) | ~USD 200–400/year | Cloud HSM or `.pfx`; SmartScreen reputation builds over time |
| **EV certificate** | ~USD 300–600/year | **Since 2024 EV no longer bypasses SmartScreen instantly** — same reputation rules as OV |
| Microsoft Store | USD 19 (individual) | Store apps are not flagged by SmartScreen |
| Self-signed | free | Internal use only (root must be deployed via GPO) — not for public releases |

> Even signed releases may be flagged until the file and certificate build
> enough SmartScreen reputation. Sign **every release with the same
> certificate** so reputation accumulates. You can also submit a build to
> [Microsoft WDSI](https://www.microsoft.com/en-us/wdsi/filesubmission/) for
> manual review.

### Option A — OV certificate with a `.pfx` (wired up in this repo)

1. Convert the certificate to `.pfx`:
   ```bash
   openssl pkcs12 -export -in cert.cer -inkey private-key.key -out certificate.pfx
   ```
2. Base64-encode (PowerShell): `certutil -encode certificate.pfx base64cert.txt`
   (strip the `-----BEGIN/END CERTIFICATE-----` lines if present).

| Secret | Value |
|---|---|
| `WINDOWS_CERTIFICATE` | Base64 contents of `certificate.pfx` |
| `WINDOWS_CERTIFICATE_PASSWORD` | Password used when exporting the `.pfx` |

The `windows-amd64` job decodes the `.pfx`, imports it into
`Cert:\CurrentUser\My`, reads the thumbprint, and builds NSIS with
`certificateThumbprint` + `digestAlgorithm: sha256` + the DigiCert timestamp
server injected into the bundle config. Tauri signs both `MieoSQL.exe` and the
`*-setup.exe` installer. Without secrets the job builds unsigned.

### Option B — Azure Artifact Signing

1. Create an account + certificate profile (see Microsoft's
   [quickstart](https://learn.microsoft.com/en-us/azure/trusted-signing/quickstart)).
2. Create an Entra ID app registration with the *Artifact Signing Certificate
   Profile Signer* role; note `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`,
   `AZURE_TENANT_ID`.
3. Add to the Windows job:
   ```yaml
   - name: Install artifact-signing-cli
     run: cargo install artifact-signing-cli
   - name: Build NSIS setup (amd64)
     working-directory: apps/mieosql-desktop
     env:
       AZURE_CLIENT_ID: ${{ secrets.AZURE_CLIENT_ID }}
       AZURE_CLIENT_SECRET: ${{ secrets.AZURE_CLIENT_SECRET }}
       AZURE_TENANT_ID: ${{ secrets.AZURE_TENANT_ID }}
     run: |
       cargo tauri build --bundles nsis --config '{"bundle":{"windows":{"signCommand":"artifact-signing-cli -e https://wus2.codesigning.azure.net -a <Account> -c <Profile> -d MieoSQL %1"}}}'
   ```

### Verify

```powershell
signtool verify /pa /v .\MieoSQL_0.1.0_x64-setup.exe   # "Successfully verified"
```
Or right-click the file → *Properties → Digital Signatures*.

---

## SignPath.io — free for open source (Windows)

SignPath Foundation issues a **free code-signing certificate** for OSS
projects. The certificate is issued to *SignPath Foundation*, signing happens
inside their HSM (the private key never leaves SignPath), and **every release
requires manual approval**. Windows only — macOS still needs Apple.

### Requirements (from signpath.org/terms) — repository status

| Requirement | Status |
|---|---|
| OSI-approved license, no commercial dual-licensing | ✅ `MIT OR Apache-2.0` (`LICENSE-MIT`, `LICENSE-APACHE`) |
| Public, actively maintained repository | ✅ |
| An actual release exists (installer) | ⚠️ push a `v*` tag so the release workflow publishes installers |
| Functionality documented on the download page | ✅ README + GitHub Releases |
| **"Code signing policy" on the project home page** | ✅ section in the README with the required wording |
| No malware / hacking tools | ✅ |
| MFA for team members | ⚠️ enable 2FA on the GitHub account |
| Builds on GitHub-hosted runners | ✅ current workflow |
| "Verifiable reputation" for executable programs | ⚠️ new projects may be questioned — have releases + a complete README before applying |

### How to request

1. Go to **https://signpath.org/apply** and fill in the form: project name,
   repository URL, license, description, download page, CI system (GitHub
   Actions), and contact details.
2. Wait for review (days to a few weeks). SignPath may ask follow-up questions
   about reputation.
3. Once approved you get an Organization, Project, signing policies
   (`test-signing` auto-approves for pipeline testing; `release-signing`
   requires an approver for every release), an Artifact Configuration, and an
   API token for CI.
4. Install the **SignPath GitHub App** (https://github.com/apps/signpath) on
   the repository for origin verification and audit logs.

### CI integration (after approval)

```yaml
- name: Upload unsigned installer
  id: upload-unsigned
  uses: actions/upload-artifact@v4
  with:
    name: windows-unsigned
    path: target/release/bundle/nsis/*-setup.exe

- name: Submit signing request
  id: signpath
  uses: signpath/github-action-submit-signing-request@v3
  with:
    api-token: ${{ secrets.SIGNPATH_API_TOKEN }}
    organization-id: "<SignPath organization id>"
    project-slug: "mieosql"
    signing-policy-slug: "release-signing"   # use test-signing first
    artifact-configuration-slug: "nsis-installer"
    github-artifact-id: ${{ steps.upload-unsigned.outputs.artifact-id }}
    wait-for-completion: true
    output-artifact-directory: signed
```

- `release-signing` waits for approval: the action prints a
  `signing-request-web-url` you open to approve (use `test-signing` to try the
  pipeline end-to-end first).
- The signed installer lands in `signed/` — upload it to the GitHub Release.

**Example artifact configuration** (`upload-artifact` stores a ZIP, so the
root element is `<zip-file>`; metadata restrictions are required by SignPath):

```xml
<artifact-configuration xmlns="http://signpath.io/artifact-configuration/v1">
  <zip-file>
    <pe-file path="*-setup.exe" product-name="MieoSQL">
      <authenticode-sign description="MieoSQL installer" />
    </pe-file>
  </zip-file>
</artifact-configuration>
```

> Signing the installer (the file users download) is what clears SmartScreen;
> the installed `.exe` carries no Mark-of-the-Web and is not flagged. To sign
> the app executable too, submit a second artifact with a similar config.

## Running unsigned builds

- **macOS**: right-click the app → **Open** → *Open* (first launch only), or
  ```bash
  xattr -dr com.apple.quarantine /Applications/MieoSQL.app
  ```
- **Windows**: SmartScreen → *More info* → *Run anyway*.

## FAQ

### Can an SSL/TLS certificate (Cloudflare, Let's Encrypt, a website's HTTPS cert) sign code?

**No.** Authenticode requires a certificate with **Extended Key Usage =
`Code Signing`**, issued by a CA in the **Microsoft Trusted Root Program**
(DigiCert, Sectigo, SSL.com, Certum, GlobalSign…). SSL/TLS certificates only
carry `TLS Server Authentication`:

- `signtool` rejects them (*"certificate is not valid for code signing"*).
- SmartScreen/Gatekeeper do not accept them as valid signatures.
- With Cloudflare Universal SSL the private key is held by Cloudflare;
  Cloudflare Origin CA is for TLS servers — **Cloudflare does not sell code
  signing certificates**.

> Don't confuse **SSL.com** — a certificate authority that *does* sell Code
> Signing certificates — with "Cloudflare SSL".

Using Cloudflare to host/cache the download page is fine, but it is
**independent** of SmartScreen: reputation is tied to the signature, not to
the download channel.

### Are there cheaper/free options?

- **Azure Artifact Signing**: ~USD 10/month — cheapest CI-friendly option.
- **SignPath.io**: free for open source projects (see above).
- **Certum**: lower-cost code signing certificates.
- **Microsoft Store**: USD 19 — Store apps are not flagged.
- **Microsoft WDSI submission**: free manual review that may add reputation.

## Notes

- The **Tauri updater** signature (`TAURI_SIGNING_PRIVATE_KEY`) is a separate
  mechanism for auto-updates and is **not related** to malware warnings. This
  project does not use the updater yet.
- Linux packages (`.deb`, `.pkg.tar.zst`) have no equivalent warning system.
- CI is wired to **enable signing automatically when the secrets exist** and
  **skip it otherwise** — no workflow changes are needed after purchasing a
  certificate.
