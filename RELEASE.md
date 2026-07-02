# Release Process

## ⚠️ IMPORTANT: Update `Cargo.toml` version FIRST!

The tag **must** match the version in `Cargo.toml` or the release will fail.

## Steps

```bash
# 1. Update version in Cargo.toml, run tests, etc.

# 2. Commit and push to main
git commit -am "release: version X.Y.Z"
git push

# 3. Push the tag (triggers cargo-dist CI)
git tag vX.Y.Z
git push --tags

# 4. Publish to crates.io (optional)
cargo publish
```
