# Greet

The CLI prints a greeting for the given name.

Covers: `src/app.sh`

## Sub-features

- `greet-name` greets the given name.

## How to get to it (user POV)

- Run `sh src/app.sh <name>`.

## Driving it with verify_pty

- Run `verify_pty --out <dir> -- sh src/app.sh cruise`; the screen shows `hello, cruise`.

## Gotchas

- none
