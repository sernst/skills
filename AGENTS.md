# AGENTS.md
## Repository map
- [`skills/`](./skills) contains reusable Markdown `SKILL.md` instructions.
- [`clis/`](./clis) contains native tools, currently
  [`skill-manager`](./clis/skill-manager).
- Additional CLIs register in [`clis/registry.just`](./clis/registry.just);
  root recipes fan out through `tools/run-registered.ps1`.
## Build and test
Run from the repository root:
```powershell
just build          # cargo build --locked
just test           # cargo test --locked --all-features
just check          # complete CI gate
```
The same recipes run directly from `clis/skill-manager`. Run `just check`
before opening a pull request.
## Authoritative skill-manager guidance
Any change to command output, prompts, plans, flags, tables, or confirmations
MUST first follow
[`docs/ux-guidelines.md`](clis/skill-manager/docs/ux-guidelines.md). It is
authoritative; do not copy an older command's presentation.

- [CLI behavior](clis/skill-manager/docs/cli.md)
- [NDJSON contract](clis/skill-manager/docs/json.md)
- [Configuration and storage](clis/skill-manager/docs/configuration.md)
- [Architecture](clis/skill-manager/docs/architecture.md)
- [Development and releases](clis/skill-manager/docs/development.md)
## Scratch-home safety
Never run a built `skill-manager` binary against the real user home during
validation. Always pass `--home <scratch-dir>`; it outranks
`SKILL_MANAGER_HOME` and the OS home.
To seed realistic scratch state, pass `FROM` as an already shell-expanded
absolute path, never literal `~` (which resolves against the active scratch
home), and keep the same absolute scratch `--home` for every command:
```powershell
$smoke = Join-Path $env:TEMP 'skill-manager-smoke'
skill-manager --home $smoke configs copy $HOME $smoke --yes
skill-manager --home $smoke status
```
