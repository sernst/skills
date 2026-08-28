# Use skill-manager through an agent

The `managing-skills` skill lets an agent operate `skill-manager` with the
same selection, preview, safety, and result semantics as the human CLI. The
agent does not install the CLI or change your persistent `PATH`.

## Quick start

1. Install `skill-manager` from the repository [README](../README.md#install-skill-manager),
   then verify it in a shell:

   ```console
   skill-manager --version
   ```

2. Register this repository's `main`-branch skills directory, preview the
   shared global deployment, apply it, and verify the result:

   ```console
   skill-manager source add https://github.com/sernst/skills/tree/main/skills --name sernst-skills --label "sernst skills"
   skill-manager load sernst-skills --filter managing-skills --shared --global --dry-run
   skill-manager load sernst-skills --filter managing-skills --shared --global
   skill-manager status managing-skills --shared --global
   ```

3. Start a new agent session if the harness discovers skills only at startup.
   Ask the agent to **manage skills**, or invoke `$managing-skills` explicitly.

For local development, register the checkout's absolute `skills` directory in
place of the GitHub URL. Keep `--name sernst-skills` so the remaining commands
stay the same.

## If the CLI is unavailable

The agent must stop and direct you to the [human installation steps](../README.md#install-skill-manager).
After installation, verify `skill-manager --version` in the agent's execution
environment and restart the agent session if necessary. The agent must not run
an installer, select an install directory, or modify persistent `PATH` on your
behalf.
