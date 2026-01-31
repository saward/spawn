---
title: CI/CD
description: Using Spawn in CI/CD pipelines with GitHub Actions.
---

Spawn provides an official GitHub Action to install the CLI in your workflows.

## GitHub Action

The [`saward/spawn-action`](https://github.com/marketplace/actions/saward-spawn) action downloads the Spawn CLI and adds it to your PATH.

### Basic usage

```yaml
- name: Install Spawn
  uses: saward/spawn-action@v1
```

### Pinning a version

```yaml
- name: Install Spawn
  uses: saward/spawn-action@v1
  with:
    version: "0.1.8"
```

Without `version`, the latest release is installed.

## Example workflow

A typical CI workflow installs Spawn, starts a database, validates migrations, and runs tests.

```yaml
name: Database Tests
on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Spawn
        uses: saward/spawn-action@v1

      - name: Start database
        run: docker compose up -d db

      - name: Check migrations
        run: spawn check

      - name: Run tests
        run: |
          spawn test compare user-accounts
          spawn test compare order-trigger
```

## Key commands for CI

### `spawn check`

Validates that all migrations are pinned. Returns a non-zero exit code if any migrations are unpinned, making it a good gate for pull requests. Consult [`spawn check`](/cli/spawn-check) for more information.

```yaml
- name: Check migrations
  run: spawn check
```

### `spawn test compare`

Runs tests and compares output against expected baselines. Fails if there are any differences.

```yaml
- name: Run tests
  run: spawn test compare my-tests
```
