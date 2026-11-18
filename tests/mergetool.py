"""Run with python3 tests/mergetool.py target/debug/dplex on Unix."""

import errno
import fcntl
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

PROJECT = Path(__file__).resolve().parents[1]
FILE = "merge target.txt"


def review_with_keys(repo, env, command, keys):
    """Send keys only after dplex enters its alternate screen and raw mode."""
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))

    def attach_terminal():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    proc = subprocess.Popen(
        command,
        cwd=repo,
        env=env,
        stdin=slave,
        stdout=slave,
        stderr=slave,
        preexec_fn=attach_terminal,
    )
    os.close(slave)
    output = bytearray()
    deadline = time.monotonic() + 10
    try:
        while (remaining := deadline - time.monotonic()) > 0:
            if not select.select([master], [], [], remaining)[0]:
                break
            try:
                chunk = os.read(master, 65536)
            except OSError as error:
                if error.errno != errno.EIO:
                    raise
                break
            if not chunk:
                break
            output.extend(chunk)
            if keys and b"\x1b[?1049h" in output:
                os.write(master, keys)
                keys = b""
        else:
            raise AssertionError(f"{command!r} timed out: {output!r}")
        status = proc.wait(timeout=max(0, deadline - time.monotonic()))
        assert not keys, f"dplex never entered review: {output!r}"
        return status, output
    finally:
        if proc.poll() is None:
            os.killpg(proc.pid, signal.SIGKILL)
            proc.wait()
        os.close(master)


def check(repo, env):
    def git(*args, expected=0):
        result = subprocess.run(
            ["git", *args],
            cwd=repo,
            env=env,
            capture_output=True,
            timeout=10,
            stdin=subprocess.DEVNULL,
        )
        assert result.returncode == expected, (args, result.stdout, result.stderr)
        return result.stdout

    git("init", "-b", "main")
    git("config", "user.name", "dplex test")
    git("config", "user.email", "test@example.invalid")
    config = (
        (PROJECT / "README.md").read_text().split("```ini\n", 1)[1].split("```", 1)[0]
    )
    with (repo / ".git/config").open("a") as f:
        f.write("\n" + config)
    assert git("config", "--get", "mergetool.dplex.cmd") == (
        b'dplex "$BASE" "$LOCAL" "$REMOTE" "$MERGED"\n'
    )
    assert (
        git("config", "--get", "--bool", "mergetool.dplex.trustExitCode").strip()
        == b"true"
    )

    target = repo / FILE
    (repo / ".gitattributes").write_text("*.txt conflict-marker-size=5\n")
    target.write_bytes(b"base\n")
    git("add", ".")
    git("commit", "-qm", "base")
    git("switch", "-qc", "incoming")
    theirs = b"their content\n"
    target.write_bytes(theirs)
    git("commit", "-qam", "theirs")
    git("switch", "-q", "main")
    target.write_bytes(b"our content\n")
    git("commit", "-qam", "ours")
    git("merge", "incoming", expected=1)
    conflict = target.read_bytes()
    assert b"<<<<< HEAD\n" in conflict and b"\n=====\n" in conflict
    stages = git("ls-files", "-u", "--", FILE)
    assert len(stages.splitlines()) == 3

    target.write_bytes(b"no conflict markers\n")
    git("mergetool", "--no-prompt", "--", FILE, expected=1)
    assert git("ls-files", "-u", "--", FILE) == stages
    target.write_bytes(conflict)

    mergetool = ["git", "mergetool", "--no-prompt", "--", FILE]
    for command in (["dplex", FILE], mergetool):
        # Git restores its backup on cancellation. The direct run also checks
        # that dplex itself discards unsaved choices.
        for keys in (b"q", b"tqd", b"tuq"):
            status, output = review_with_keys(repo, env, command, keys)
            assert status != 0, (keys, output)
            assert target.read_bytes() == conflict
            assert git("ls-files", "-u", "--", FILE) == stages

    status, output = review_with_keys(repo, env, mergetool, b"tSalternate.txt\r\r")
    assert status != 0, output
    assert (repo / "alternate.txt").read_bytes() == theirs
    assert target.read_bytes() == conflict
    assert git("ls-files", "-u", "--", FILE) == stages

    status, output = review_with_keys(repo, env, mergetool, b"t\r")
    assert status == 0, output
    assert git("ls-files", "-u", "--", FILE) == b""
    assert git("show", f":{FILE}") == target.read_bytes() == theirs


if __name__ == "__main__":
    binary = Path(sys.argv[1]).resolve(strict=True)
    with tempfile.TemporaryDirectory() as temp:
        root = Path(temp)
        repo = root / "repo"
        repo.mkdir()
        (root / "bin").mkdir()
        (root / "bin/dplex").symlink_to(binary)
        check(
            repo,
            {
                "PATH": f"{root / 'bin'}{os.pathsep}{os.defpath}",
                "HOME": temp,
                "GIT_CONFIG_NOSYSTEM": "1",
                "GIT_ATTR_NOSYSTEM": "1",
                "TERM": "xterm",
                "LC_ALL": "C",
            },
        )
    print(
        "Git mergetool preserved unmerged stages on failure and staged the selected bytes on success."
    )
