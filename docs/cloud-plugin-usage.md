# Use these skills as a plugin in other repositories

This repository publishes [`.claude-plugin/marketplace.json`](../.claude-plugin/marketplace.json),
which exposes the entire [`skills/`](../skills) directory as a single Claude
Code plugin named `skills` in the `sernst-skills` marketplace. Declaring that
marketplace in another repository's committed `.claude/settings.json` is the
supported way to make these skills available there, including in
[Claude Code cloud/web sessions](https://code.claude.com/docs/en/claude-code-on-the-web),
without uploading anything or committing a copy of the skill content that can
go stale.

## Why this instead of a manual copy or upload

- **Cloud and Cowork sessions don't read `~/.claude/skills/` on your
  machine** — they run on a different machine entirely, so nothing you set up
  locally reaches them.
- **"Enable skill for your claude.ai account" is a one-time upload**, not a
  link to a git source. Every update to a skill here would need a manual
  re-upload.
- **A plugin declared in a repo's project-scoped `.claude/settings.json` is
  different**: it's a pointer (marketplace name + plugin name), not a copy of
  the content. Claude Code cloud sessions install/refresh repo-declared
  plugins at session start, so every session re-resolves against this
  repository's current default branch. Push a skill update here, and the next
  session anywhere that has this marketplace declared picks it up — no
  re-upload, no stale per-repo copy.
- Only **project scope** (checked into the repo) reaches cloud sessions this
  way. A marketplace/plugin enabled only in your personal, machine-local user
  scope (`~/.claude/settings.json`) does not transfer to cloud sessions.

## Add it to another repository

Merge the following into that repository's `.claude/settings.json` (create
the file if it doesn't exist yet; merge these keys into it if it already has
other settings such as `hooks` or other `enabledPlugins`/
`extraKnownMarketplaces` entries — don't overwrite the file):

```json
{
  "extraKnownMarketplaces": {
    "sernst-skills": {
      "source": {
        "source": "github",
        "repo": "sernst/skills"
      }
    }
  },
  "enabledPlugins": {
    "skills@sernst-skills": true
  }
}
```

Commit and push that change. From then on, any Claude Code session in that
repository — local, cloud, or a [routine](https://code.claude.com/docs/en/routines)
— loads the `skills` plugin, namespaced as `skills:<skill-name>` (for example
`skills:drafting-commit-message`).

### Ask an agent to do it

Point an agent working in the target repository at this file and ask it to
apply the change, for example:

> Read
> https://raw.githubusercontent.com/sernst/skills/main/docs/cloud-plugin-usage.md
> and add the `sernst-skills` plugin marketplace to this repo's
> `.claude/settings.json`, merging it with whatever is already there.

The JSON above is the complete, exact change; an agent needs nothing else
from this repository to apply it.

### Verify

- **Locally**: run `/plugin marketplace list` (expect `sernst-skills`) and
  `/plugin list --enabled` (expect `skills@sernst-skills`). If the install
  summary said `Run /reload-plugins to activate.`, run that too.
- **In a cloud/web session**: the plugin installs automatically at session
  start once the settings change is committed; ask Claude to use one of the
  skills (for example `$skills:managing-skills`) to confirm it loaded.

### Try it without committing anything

To try a skill interactively in a local terminal session first:

```console
/plugin marketplace add sernst/skills
/plugin install skills@sernst-skills
```

This only affects your personal user-scope settings on that machine, so it
won't reach cloud sessions — commit the `.claude/settings.json` change above
once you're ready to make it available there too.

## Updating

Nothing to do on the consuming side. This repository's `skills/` directory is
the live source; the marketplace always resolves against its current default
branch. If a local session has an older cached copy, refresh it with
`/plugin marketplace update sernst-skills`.
