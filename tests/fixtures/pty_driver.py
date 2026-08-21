#!/usr/bin/env python3
"""PTY driver for the interactive-mode e2e tests.

Runs the given command inside a real pseudo-terminal (reedline requires
one), then executes a scripted dialogue: for each (expect, prompt) pair it
waits until the prompt is open, types, and waits until the expected text
appears in output produced AFTER the prompt was sent. Finally types /exit
and waits for a clean child exit. The child must exit on its own — killing
it would drop its LLVM coverage profile.

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
    prompts_consumed = 0

    # reedline turns bracketed paste on at the start of every read_line, and
    # octomind flushes the tty input queue just before that (term_echo::
    # drain_stdin, so keypresses from the spinner phase don't land in the next
    # prompt). Anything typed before this marker is therefore thrown away —
    # every write below waits for a marker it hasn't used yet.
    PROMPT_READY = b"\x1b[?2004h"

    def answer_dsr():
        # reedline probes the cursor position with ESC[6n and waits for the
        # ESC[row;colR reply — a real terminal answers, so we must too.
        nonlocal dsr_replies
        queries = transcript.count(b"\x1b[6n")
        while dsr_replies < queries:
            os.write(fd, b"\x1b[1;1R")
            dsr_replies += 1

    def pump():
        """Absorb one chunk of child output. False once the pty is done."""
        nonlocal transcript
        ready, _, _ = select.select([fd], [], [], 0.2)
        if not ready:
            return True
        try:
            chunk = os.read(fd, 65536)
        except OSError:
            return False
        if not chunk:
            return False
        transcript += chunk
        answer_dsr()
        return True

    def read_until(needle, start, timeout):
        end = time.time() + timeout
        while time.time() < end:
            if needle.encode() in transcript[start:]:
                return True
            if not pump():
                return False
        return False

    def wait_for_prompt(timeout):
        """Block until reedline opens a prompt we haven't typed into yet."""
        nonlocal prompts_consumed
        end = time.time() + timeout
        while time.time() < end:
            seen = transcript.count(PROMPT_READY)
            if seen > prompts_consumed:
                prompts_consumed = seen
                return True
            if not pump():
                return False
        return False

    def give_up(reason, code):
        sys.stderr.write(transcript.decode(errors="replace"))
        sys.stderr.write(f"\n--- {reason} ---\n")
        try:
            os.kill(pid, 9)
            os.waitpid(pid, 0)
        except (ProcessLookupError, ChildProcessError):
            pass  # the child died on its own — the transcript above says why
        sys.exit(code)

    try:
        for expect_text, prompt_text in pairs:
            if not wait_for_prompt(deadline - time.time()):
                give_up(f"no prompt to type {prompt_text!r} into", 1)
            start = len(transcript)
            os.write(fd, prompt_text.encode() + b"\r")
            if not read_until(expect_text, start, deadline - time.time()):
                give_up(f"{expect_text!r} never appeared after {prompt_text!r}", 1)

        # All steps done — exit the session cleanly.
        if not wait_for_prompt(deadline - time.time()):
            give_up("no prompt to type '/exit' into", 2)
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
