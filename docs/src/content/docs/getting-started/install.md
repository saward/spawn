---
title: Install Spawn
description: How to install Spawn.
---

## Shell Installer (macOS/Linux)

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/saward/spawn/releases/latest/download/spawn-db-installer.sh | sh
```

## Using Cargo

If you have Rust installed, you can install Spawn directly from crates.io:

```bash
cargo install spawn-db
```

Or install from the GitHub repository:

```bash
cargo install --git https://github.com/saward/spawn
```

## Using Homebrew (macOS)

```bash
brew install saward/tap/spawn-db
```

## Pre-built Binaries

Download the latest release for your platform from [GitHub Releases](https://github.com/saward/spawn/releases/latest).
