# Publishing a Mac release

Downloads live in [GitHub Releases](https://github.com/simpsoka/zigzag/releases).
The README uses a stable `releases/latest/download/ZigZag.dmg` link; every
release includes that filename, a versioned DMG, and `SHA256SUMS.txt`.
Build products stay in the ignored `dist/` directory, outside Git history.

## One-time setup

Install Rust, Xcode command line tools, and the [GitHub CLI](https://cli.github.com/).
Run `gh auth login` for an account with write access to `simpsoka/zigzag`.
Set up your Developer ID certificate and the `zigzag-notary` keychain profile
as described in the README's **Mac app** section. Signing credentials stay in
your Mac's keychain.

## Publish

1. Update `version` in `Cargo.toml` and regenerate `Cargo.lock` with Cargo.
   If the lockfile changes, regenerate `THIRD-PARTY-NOTICES.md` with
   `packaging/licenses/generate.sh`.
2. Add `packaging/releases/<version>.md` with the release notes.
3. Test the app, commit all changes, and push `main` to GitHub.
4. Run:

   ```sh
   bash packaging/macos/publish-release.sh
   ```

The command builds a universal, signed, notarized DMG, verifies it, tags the
pushed commit, uploads the assets to a draft release, then publishes it as
the latest release. It requires a clean checkout on `main` with `origin`
pointing to this repository. It never replaces an existing release or tag.

To publish a DMG already built from the current commit:

```sh
bash packaging/macos/publish-release.sh --existing
```

Only use `--existing` when `dist/ZigZag-<version>.dmg` was built from the
current source. It verifies the image and Apple's notarization ticket, but
cannot establish which source commit produced an existing binary.

If an upload fails, the release stays a draft. Finish uploading the two DMGs
and `SHA256SUMS.txt` in GitHub's release editor before publishing it. If only
the tag was pushed, create the release from that tag in GitHub. The script
deliberately refuses to overwrite either.

## Verify a download

Download a DMG and `SHA256SUMS.txt` from the same release, then run:

```sh
shasum -a 256 --ignore-missing -c SHA256SUMS.txt
```

The two DMG filenames contain identical bytes; either can be used to install.
