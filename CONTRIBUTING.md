# Contributing to engram

Thanks for your interest in improving engram.

## Getting started

```bash
git clone https://github.com/hdot123-org/engram.git
cd engram
npm install
npm test
```

## Before opening a pull request

1. `npm run typecheck` passes.
2. `npm test` passes (add tests for any new behavior).
3. `npm run build` succeeds.
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

Open an issue with: the engram version, runtime (browser/Node/edge), a minimal reproduction, and what you expected instead.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](LICENSE).
