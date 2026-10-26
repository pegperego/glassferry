#!/usr/bin/env python3
"""ProxyCommand that adds one-way latency, to test SFTP pipelining.

Usage in ssh_config: ProxyCommand python3 delay-relay.py <host> <port> <delay-ms>
Data is delayed, not throttled: each chunk is released <delay-ms> after it
was received, like a long network path.
"""
import socket
import sys
import threading
import time
from collections import deque

host, port, delay = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]) / 1000.0
sock = socket.create_connection((host, port))


def pump(read, write, close):
    queue = deque()
    cond = threading.Condition()
    done = [False]

    def reader():
        while True:
            data = read()
            with cond:
                if not data:
                    done[0] = True
                else:
                    queue.append((time.monotonic() + delay, data))
                cond.notify()
            if not data:
                return

    threading.Thread(target=reader, daemon=True).start()
    while True:
        with cond:
            while not queue and not done[0]:
                cond.wait()
            if not queue and done[0]:
                close()
                return
            due, data = queue.popleft()
        wait = due - time.monotonic()
        if wait > 0:
            time.sleep(wait)
        write(data)


stdin, stdout = sys.stdin.buffer.raw, sys.stdout.buffer.raw


def write_stdout(data):
    view = memoryview(data)
    while view:
        n = stdout.write(view)
        view = view[n:]


up = threading.Thread(
    target=pump,
    args=(lambda: stdin.read(65536), sock.sendall, lambda: sock.shutdown(socket.SHUT_WR)),
    daemon=True,
)
up.start()
pump(lambda: sock.recv(65536), write_stdout, lambda: None)
