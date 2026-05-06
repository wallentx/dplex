# dplex fixtures

These files are intentionally conflicted so you can exercise `dplex` by hand.
Because `dplex` writes reviewed decisions back to the merged file, copy a
fixture before running it.

Single-file mode:

```sh
mkdir -p target/fixtures
cp fixtures/single/simple.txt target/fixtures/simple.txt
cargo run -- target/fixtures/simple.txt
```

Multiple-conflict mode:

```sh
mkdir -p target/fixtures
cp fixtures/single/multiple.txt target/fixtures/multiple.txt
cargo run -- target/fixtures/multiple.txt
```

Diff3 marker mode:

```sh
mkdir -p target/fixtures
cp fixtures/single/diff3.txt target/fixtures/diff3.txt
cargo run -- target/fixtures/diff3.txt
```

Two-file diff mode:

```sh
mkdir -p target/fixtures
cp fixtures/pair/configA.yaml target/fixtures/configA.yaml
cargo run -- target/fixtures/configA.yaml fixtures/pair/configB.yaml
```

In two-file mode, each changed region is reviewed like a conflict. Choosing
ours preserves `target/fixtures/configA.yaml`; choosing theirs takes the
matching region from `fixtures/pair/configB.yaml`. Saves write to the first file
by default.

Git mergetool argument shape:

```sh
mkdir -p target/fixtures
cp fixtures/mergetool/merged.txt target/fixtures/merged.txt
cargo run -- \
  fixtures/mergetool/base.txt \
  fixtures/mergetool/local.txt \
  fixtures/mergetool/remote.txt \
  target/fixtures/merged.txt
```

The configured Git mergetool command this mirrors is:

```ini
[merge]
    tool = dplex

[merge.tool "dplex"]
    cmd = dplex "$BASE" "$LOCAL" "$REMOTE" "$MERGED"
    trustExitCode = true

[mergetool]
    keepBackup = false
```
