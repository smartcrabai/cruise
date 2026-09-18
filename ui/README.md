# React + TypeScript + Vite

This template provides a minimal setup to get React working in Vite with HMR and some lint rules.

Currently, two official plugins are available:

- [@vitejs/plugin-react](https://github.com/vitejs/vite-plugin-react/blob/main/packages/plugin-react) uses [Oxc](https://oxc.rs)
- [@vitejs/plugin-react-swc](https://github.com/vitejs/vite-plugin-react/blob/main/packages/plugin-react-swc) uses [SWC](https://swc.rs/)

## React Compiler

The React Compiler is not enabled on this template because of its impact on dev & build performances. To add it, see [this documentation](https://react.dev/learn/react-compiler/installation).

## Linting

Linting uses [oxlint](https://oxc.rs/docs/guide/usage/linter.html); the rule set lives in `.oxlintrc.json`.
ESLint and `typescript-eslint` were removed because `typescript-eslint` cannot run against
TypeScript 7 (it has no stable JS API); oxlint is a native binary with no `typescript` dependency.

```sh
pnpm lint       # oxlint .
pnpm lint:fix   # oxlint . --fix
```

`.oxlintrc.json` was generated with `npx @oxlint/migrate` from the former `eslint.config.js`, so it
mirrors `@eslint/js` recommended, `typescript-eslint` recommended, `eslint-plugin-react-hooks`
(including the React Compiler rules) and `eslint-plugin-react-refresh`.

Type-aware rules are not enabled. They require the extra `oxlint-tsgolint` binary and are the
equivalent of `tseslint.configs.recommendedTypeChecked`; `tsc -b` (run by `pnpm build`) already
covers type checking.
