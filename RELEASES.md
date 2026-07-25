# Releases

Releases are created only from annotated tags named `vMAJOR.MINOR.PATCH` (with
an optional SemVer prerelease suffix). The tag must point to a commit reachable
from `origin/main`; build metadata is intentionally not accepted.

Update `VERSION`, each CLI package version, and the matching `CHANGELOG.md`
heading together. Then run `just release-check`. It verifies a clean tree,
version parity, and main ancestry, creates the annotated tag, and prints the
push command without pushing it. Pushing the tag starts the immutable draft
release workflow. The workflow validates and packages every registered CLI for
all supported targets before publishing the GitHub release.

Manual workflow dispatch builds and retains artifacts but never publishes a
release. This repository does not publish crates to crates.io.
