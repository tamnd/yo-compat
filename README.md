# yo-compat

Redis compatibility for [yo](https://github.com/tamnd/yo), checked against a real Redis rather than against the documentation.

There are two halves here and they catch different things.

## The real Redis test suite

`suite/fetch.sh` downloads the Redis release tarball at a pinned version and drops its `tests/` directory and its `runtest` script into `redis/` unmodified. Not a rewrite of the suite, not a subset we typed out ourselves, and not a description of what we think it checks. It is their tests, run against our server.

```
suite/fetch.sh
suite/runtest.sh
```

That starts `yodb serve` and points the suite at it with `runtest --host --port`, which is the suite's external mode. With no arguments it runs the files listed in `suite/passing.txt`, and `--all` runs everything under `unit/`.

Every failure is real. A test that fails because we have not implemented the command yet is still a compatibility failure, it is just one with a date on it. Nothing is patched out to make the number look better, and moving a file into `suite/passing.txt` is a decision somebody makes on purpose in a diff.

## The differential harness

`yocompat` starts a real Redis and starts yodb, sends both of them the same commands, and reports every reply that is not byte for byte the same.

```
cargo build --release
./target/release/yocompat --redis /opt/yo-bench/bin/redis-server-8.10.1
./target/release/yocompat --resp3
```

This catches what their suite cannot. Their tests assert what Redis's own developers thought was worth asserting, which leaves out everything they take for granted: that a missing key comes back as `$-1` and not `_`, that `INCRBYFLOAT` replies with a bulk string and not a double, that `GETRANGE` on a missing key is an empty bulk string and not a null, that the first word of an error is `WRONGTYPE` and not `ERR`. Clients branch on all of it.

The corpus files under `corpus/` are lists of commands with no expected answers in them, because the expected answer is whatever Redis says. `--resp3` sends `HELLO 3` first and runs the same corpus again, which is where the reply types that change between protocol versions show up.

Divergences are sorted worst first in the report. Accepting a command Redis refuses is a correctness bug that will corrupt data for a client that relied on the refusal. Refusing it with different wording is a bug too, but a smaller one.

## The register

`divergences.toml` is the list of differences we have decided to live with, each with a reason written next to it. Anything in it is counted and not reported. Anything else fails the run.

The file is deliberately awkward to add to. A divergence with no reason on it does not load at all, which means saying "we behave differently here and this is why" has to be a diff somebody wrote and somebody else read.

## Where the suite stands

Run `suite/sweep.sh` and it runs every file on its own, with its own server and its own time limit, and prints what each one did. This is the state on 2026-08-29, against yo at the head of main and the Redis 8.10.1 tests, on a Linux box.

Two files pass end to end and are in `suite/passing.txt`: `unit/networking` at 1 of 1 and `unit/quit` at 3 of 3.

Eight more run real tests and then stop at a command we have not written yet. `unit/protocol` is 14 passed and 3 failed and ends at `DEBUG PROTOCOL`, where the three failures are `SADD` and `SRANDMEMBER`. `unit/type/incr` is 11 and ends at `RPUSH`. `unit/type/increx` is 11 and ends at `OBJECT ENCODING`. `unit/keyspace` is 3 and ends at `KEYS`. `unit/type/string` is 2 and ends at `proto-max-bulk-len`. `unit/introspection` is 1 and ends at `CLIENT`. `unit/info-command` runs to the end with 3 failures, which are `rejected_calls` and `master_repl_offset`. The rest end on the first command from a milestone that has not started.

Eighteen files run to the end having skipped every test in them. In external mode the suite marks a test that needs a server it started itself as `[ignore]`, so the file says nothing about us either way. The sweep calls that "nothing ran" rather than "clean", and those files are not in `passing.txt`.

A long tail of files ends on `CONFIG SET` rather than on a data command, because the suite sets a parameter Redis has and we do not before it runs anything. That is worth knowing separately from the command gaps: those files are not testing configuration, they just cannot get to the part that tests anything else.

## What is not here yet

The command scoreboard, which is the other thing this repository is supposed to hold: a generated table of every Redis command against what yo does with it. Right now the honest version of that table is short, and it belongs here rather than in a README that can go stale quietly.

Also the RDB corpus and the client library matrix. Both wait on yo having something to load and something for a real client to talk to.
