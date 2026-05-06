# dplex

<img width="1343" height="1642" alt="1000035721" src="https://github.com/user-attachments/assets/5a551739-9227-4389-996b-214a49803237" />


`dplex` is an interactive terminal merge conflict reviewer.

It lets you review hunks ( ͡° ͜ʖ ͡°)

Pass it a file that contains Git conflict markers:

```sh
dplex path/to/conflicted-file
```

Or compare two files directly:

```sh
dplex configA.yaml configB.yaml
```

Two-file mode reviews changed regions as conflicts and writes to the first file
by default.

For each conflict, `dplex` shows ours and theirs inline. Press `o` to choose
ours, `t` to choose theirs, `Ctrl+s` to save, `S` to save as, or `q`, `Esc`, or
`Ctrl+C` to stop. Chosen hunks update the document view in place; unsaved chosen
text is highlighted yellow. Unreviewed conflicts are left untouched in
conflict-file mode and keep ours in two-file mode.

Useful Git mergetool starting point:

```ini
[merge]
    tool = dplex

[merge.tool "dplex"]
    cmd = dplex "$BASE" "$LOCAL" "$REMOTE" "$MERGED"
    trustExitCode = true

[mergetool]
    keepBackup = false
```

`dplex` also supports direct single-file use, so `dplex "$MERGED"` works too.

## Fixtures

The `fixtures/` directory contains conflicted files you can copy and run
against while tuning the UI:

```sh
mkdir -p target/fixtures
cp fixtures/single/simple.txt target/fixtures/simple.txt
cargo run -- target/fixtures/simple.txt
```

For the Git mergetool argument shape:

```sh
mkdir -p target/fixtures
cp fixtures/mergetool/merged.txt target/fixtures/merged.txt
cargo run -- fixtures/mergetool/base.txt fixtures/mergetool/local.txt fixtures/mergetool/remote.txt target/fixtures/merged.txt
```

For two-file diff mode:

```sh
mkdir -p target/fixtures
cp fixtures/pair/configA.yaml target/fixtures/configA.yaml
cargo run -- target/fixtures/configA.yaml fixtures/pair/configB.yaml
```
