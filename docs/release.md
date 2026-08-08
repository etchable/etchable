# Release setup

Releases are cut from the Actions tab: run **bump-release**, pick patch/minor/major, and it bumps the version, tags `vX.Y.Z`, and pushes. The tag triggers **release-app**, which builds, signs, notarizes, and attaches the `.dmg` to a GitHub Release.

Before the first release, the repo needs eight secrets (**Settings → Secrets and variables → Actions → New repository secret** on `etchable/etchable`, or `gh secret set NAME` from this repo). Here's how to get each value.

## 1. GitHub App token (`RELEASE_APP_ID`, `RELEASE_APP_PRIVATE_KEY`)

The bump workflow pushes a tag, and pushes made with the default `GITHUB_TOKEN` deliberately don't trigger other workflows — so the tag would never kick off the release build. A GitHub App token avoids that.

If you already have a release App (e.g. the one restorekit uses), skip to installation.

1. Go to <https://github.com/settings/apps> (or your org's App settings) → **New GitHub App**.
   - Name: anything (e.g. `etchable-release`).
   - Homepage URL: anything valid.
   - Uncheck **Webhook → Active**.
   - Repository permissions: **Contents: Read and write**. Nothing else.
2. Create the App, note the **App ID** shown on its settings page → this is `RELEASE_APP_ID`.
3. On the same page, **Private keys → Generate a private key**. A `.pem` downloads → its full contents (including the `BEGIN`/`END` lines) is `RELEASE_APP_PRIVATE_KEY`.
4. **Install App** (left sidebar) → install it on the `etchable/etchable` repository.

```sh
gh secret set RELEASE_APP_ID --body "<app id>"
gh secret set RELEASE_APP_PRIVATE_KEY < path/to/etchable-release.private-key.pem
```

## 2. Developer ID certificate (`APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`)

The CI runner gets a clean keychain, so the signing cert has to be shipped to it as a password-protected `.p12`, base64-encoded.

1. Open **Keychain Access** on the Mac that has the cert.
2. Find **Developer ID Application: Left Shift Logical, LLC (KNBPD99JQM)** under *My Certificates*. There are currently two with this name — use the newer one (issued Jul 2026). Expand it and confirm it has a private key attached.
3. Right-click the certificate → **Export** → format **Personal Information Exchange (.p12)**. Set a strong password when prompted — that password is `APPLE_CERTIFICATE_PASSWORD`.
4. Base64-encode the file for the secret:

```sh
base64 -i Certificates.p12 | gh secret set APPLE_CERTIFICATE
gh secret set APPLE_CERTIFICATE_PASSWORD --body "<the export password>"
gh secret set APPLE_SIGNING_IDENTITY --body "Developer ID Application: Left Shift Logical, LLC (KNBPD99JQM)"
```

The identity name is unambiguous in CI because the runner keychain only contains the one imported cert (locally we have to pin by hash, but that doesn't apply here).

## 3. App Store Connect API key (`APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_KEY_P8_BASE64`)

Notarization authenticates with an App Store Connect API key.

1. Go to <https://appstoreconnect.apple.com/access/integrations/api> (Users and Access → Integrations → App Store Connect API) signed in to the Left Shift Logical account.
2. Note the **Issuer ID** at the top of the page → `APPLE_API_ISSUER` (a UUID).
3. **Generate API Key** (the "Team Keys" tab). Role: **Developer** is sufficient for notarization.
4. The **Key ID** of the new key → `APPLE_API_KEY` (10 characters, e.g. `ABC123DEFG`).
5. Download the `.p8` file — Apple only offers the download **once**, so store it somewhere safe (password manager).
6. Base64-encode it for the secret (single-line base64 gets masked reliably in logs; the raw multi-line PEM does not):

```sh
gh secret set APPLE_API_ISSUER --body "<issuer uuid>"
gh secret set APPLE_API_KEY --body "<key id>"
base64 -i AuthKey_ABC123DEFG.p8 | gh secret set APPLE_API_KEY_P8_BASE64
```

## Verify

With all eight secrets set:

```sh
gh secret list
```

should show: `RELEASE_APP_ID`, `RELEASE_APP_PRIVATE_KEY`, `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_KEY_P8_BASE64`.

Then run **bump-release** from the Actions tab with `patch` to cut `v0.1.1`. Watch the tag's **release-app** run; when it finishes, the GitHub Release should have a `.dmg` whose signature and notarization you can spot-check locally:

```sh
spctl -a -t open --context context:primary-signature -v etchable_0.1.1_aarch64.dmg
```

## Troubleshooting

- **bump-release pushes but no release build starts** — the GitHub App isn't installed on this repo, or the workflow fell back to `GITHUB_TOKEN`. Check the "Generate GitHub App token" step.
- **`security: SecKeychainItemImport: MAC verification failed`** — wrong `APPLE_CERTIFICATE_PASSWORD`, or the base64 got mangled (re-run the `base64 | gh secret set` pipe; don't copy-paste the output).
- **Signing succeeds but notarization hangs or 401s** — Key ID / Issuer ID swapped, or the `.p8` base64 is bad. `notarytool` errors are printed in the "Notarize & staple" step logs.
