#!/usr/bin/env python3
"""PTY driver for the interactive-mode e2e tests.

Runs the given command inside a real pseudo-terminal (reedline requires
one), then executes a scripted dialogue: for each (expect, prompt) pair it
types the prompt and waits until the expected text appears in output
produced AFTER the prompt was sent. Finally types /exit and waits for a
clean child exit. The child must exit on its own — killing it would drop
its LLVM coverage profile.

Usage: pty_driver.py <expect1> <prompt1> [<expect2> <prompt2> ...] -- <command> [args...]

Prints the full transcript to stderr and PTY_OK to stdout on success.
Exit codes: 0 success, 1 an expected text never appeared, 2 child died
early or did not exit after /exit.
"""

import os
import pty
import select
import sys
import time


def main():
    sep = sys.argv.index("--")
    flat = sys.argv[1:sep]
    command = sys.argv[sep + 1 :]
    pairs = list(zip(flat[0::2], flat[1::2]))
    assert pairs, "need at least one (expect, prompt) pair"

    pid, fd = pty.fork()
    if pid == 0:
        os.environ["TERM"] = "xterm-256color"
        os.execvp(command[0], command)

    transcript = b""
    deadline = time.time() + 120
    dsr_replies = 0

    def answer_dsr():
        # reedline probes the cursor position with ESC[6n and waits for the
        # ESC[row;colR reply — a real terminal answers, so we must too.
        nonlocal dsr_replies
        queries = transcript.count(b"\x1b[6n")
        while dsr_replies < queries:
            os.write(fd, b"\x1b[1;1R")
            dsr_replies += 1

    def read_until(needle, start, timeout):
        nonlocal transcript
        end = time.time() + timeout
        while time.time() < end:
            if needle.encode() in transcript[start:]:
                return True
            ready, _, _ = select.select([fd], [], [], 0.2)
            if not ready:
                continue
            try:
                chunk = os.read(fd, 65536)
            except OSError:
                return False
            if not chunk:
                return False
            transcript += chunk
            answer_dsr()
        return False

    try:
        # Give the session a moment to boot and paint its prompt, then type.
        time.sleep(2.0)
        for expect_text, prompt_text in pairs:
            start = len(transcript)
            os.write(fd, prompt_text.encode() + b"\r")
            if not read_until(expect_text, start, deadline - time.time()):
                sys.stderr.write(transcript.decode(errors="replace"))
                sys.stderr.write(f"\n--- {expect_text!r} never appeared after {prompt_text!r} ---\n")
                os.kill(pid, 9)
                os.waitpid(pid, 0)
                sys.exit(1)
            time.sleep(0.5)

        # All steps done — exit the session cleanly.
        os.write(fd, b"/exit\r")

        # Drain output until the child exits on its own.
        exit_deadline = time.time() + 30
        while time.time() < exit_deadline:
            done, status = os.waitpid(pid, os.WNOHANG)
            if done == pid:
                sys.stderr.write(transcript.decode(errors="replace"))
                if os.waitstatus_to_exitcode(status) == 0:
                    print("PTY_OK")
                    sys.exit(0)
                sys.stderr.write(f"\n--- child exited with {status} ---\n")
                sys.exit(2)
            ready, _, _ = select.select([fd], [], [], 0.2)
            if ready:
                try:
                    chunk = os.read(fd, 65536)
                    if chunk:
                        transcript += chunk
                        answer_dsr()
                except OSError:
                    pass

        sys.stderr.write(transcript.decode(errors="replace"))
        sys.stderr.write("\n--- child did not exit after /exit ---\n")
        os.kill(pid, 9)
        os.waitpid(pid, 0)
        sys.exit(2)
    finally:
        try:
            os.close(fd)
        except OSError:
            pass


if __name__ == "__main__":
    main()
