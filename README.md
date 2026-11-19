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

Use `o` for ours and `t` for theirs. The arrow keys move between hunks
(left/right) and scroll the document (up/down). The `h` and `l` keys scroll
long lines horizontally. You can revisit any hunk and press `u` to undo the
most recent choice.

`Ctrl+S` saves a checkpoint, and `S` lets you choose an output path. Pending
hunks keep their conflict markers in single-file mode and their original
content in two-file mode. After reviewing every hunk, press Enter to save and
finish. If you have unsaved choices, `q`, Esc, and `Ctrl+C` prompt you to
save, discard changes since the last checkpoint, or continue reviewing.

Two-file review includes line endings and the final newline in each choice.

Useful Git mergetool starting point:

```ini
[merge]
    tool = dplex

[mergetool "dplex"]
    cmd = dplex \"$BASE\" \"$LOCAL\" \"$REMOTE\" \"$MERGED\"
    trustExitCode = true

[mergetool]
    keepBackup = false
```

`dplex` also supports direct single-file use with `dplex "$MERGED"`. The four-argument
command above reports failure to Git when the merged file contains no recognized
conflicts, the saved review is incomplete, or the result was saved elsewhere.

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
