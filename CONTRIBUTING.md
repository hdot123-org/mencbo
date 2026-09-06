# Contributing to MenCbo

Thanks for your interest in improving MenCbo.

## Getting started

```bash
git clone https://github.com/hdot123-org/mencbo.git
cd mencbo
pnpm install
pnpm -F mencbo run test
```

## Before opening a pull request

Changes under `packages/mencbo/`:

1. `pnpm -F mencbo run typecheck` passes.
2. `pnpm -F mencbo run test` passes (add tests for any new behavior).
3. `pnpm -F mencbo run build` succeeds.

Changes under `apps/desktop/daemon/`:

1. `uv sync` then `uv run pytest` passes (add tests for any new behavior).
4. No new runtime dependencies — the zero-dependency core is a design guarantee.
5. Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/) (e.g. `feat:`, `fix:`, `docs:`).

## Design invariants

Keep these intact when proposing changes:

- **Project-first routing** — global layer is opt-in fallback, never primary.
- **Read-first mutations** — `update`/`delete` require `ifMatch` or explicit `force`.
- **Fail-closed guard** — guard failures deny the operation.
- **Forward-only migrations** — never write code that downgrades entry schema versions.
- **Isomorphic core** — no Node-only or browser-only APIs in `src/` without an adapter boundary.

## Reporting bugs

Open an issue with: the MenCbo version, runtime (browser/Node/edge), a minimal reproduction, and what you expected instead.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](LICENSE).
